//! Live pairing transport: drives the PAKE handshake and moves one sealed
//! credential bundle over a byte stream, in either role.
//!
//! This is the seam the LAN socket calls into (ADR-0008). It is generic over
//! any [`tokio::io::AsyncRead`] + [`tokio::io::AsyncWrite`] stream, so the
//! socket adapter stays thin — it binds, dials, and hands the stream here —
//! and the whole exchange is testable against an in-process duplex or a
//! loopback TCP connection with no network at all.
//!
//! # Wire protocol (initiator = the sender of the bundle)
//!
//! 1. Initiator → responder: 32-byte PAKE initiator message.
//! 2. Responder derives the channel key → initiator: 64-byte PAKE response
//!    (public share || confirmation tag).
//! 3. Initiator verifies the tag. A mismatch means the two sides entered
//!    different codes; the initiator aborts and sends nothing more.
//! 4. Initiator → responder: `u32` big-endian length, then that many bytes of
//!    the bundle sealed under the channel key with
//!    [`crate::seal::seal_with_key`] — the same sealed format as the file
//!    export, only with a PAKE-derived key instead of a passphrase.
//! 5. Responder opens the sealed bundle. On success it sends a single `1`
//!    byte and returns the bundle for the caller to apply; on failure it
//!    sends `0` (best effort) and reports [`PairingError::Mismatch`].
//!
//! # Attempt and reuse rules
//!
//! These functions run **exactly one** handshake and move **at most one**
//! bundle per stream — there is no retry on the same connection, so a wrong
//! code costs an attacker exactly one online guess (see [`crate::pake`]'s
//! protocol notes). Single-use of the *code itself* is the adapter's job: one
//! listen session consumes one code, whatever its outcome, and the adapter
//! must not re-offer it.

use crate::pake::{PakeInitiator, PakeResponder, KEY_LEN, RESPONSE_LEN};
use crate::seal;
use crate::transfer::CredentialBundle;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The pairing code is a 6-digit decimal string, matching the format the PAKE
/// documentation fixes. Enforced here so a transport cannot quietly run the
/// handshake with a weaker secret than the one-guess-per-attempt model
/// assumes.
pub const CODE_LEN: usize = 6;

/// Upper bound on one incoming sealed bundle. Real bundles are kilobytes; the
/// cap exists so a hostile peer cannot name a multi-gigabyte length and make
/// the receiver allocate it before any payload byte arrives.
pub const MAX_BUNDLE_LEN: u32 = 16 * 1024 * 1024;

/// How long any single step of the exchange may sit still before the session
/// is abandoned. A peer that hangs mid-handshake or mid-transfer must not
/// wedge the other side forever: a stalled step is treated the same as a
/// hang-up.
pub const STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a pairing attempt failed. Deliberately coarse: every transport-level
/// ailment (hang-up, timeout, reset) reads the same to a user standing between
/// two devices — "the transfer did not complete" — while the two cases they
/// can act on, a mistyped code and a mistyped address, get their own variants.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PairingError {
    #[error("the pairing code must be exactly {CODE_LEN} digits")]
    BadCode,
    #[error("the pairing code did not match, or the transfer was tampered with")]
    Mismatch,
    #[error("the other device hung up, stalled, or was unreachable")]
    Interrupted,
    #[error("the other device offered a transfer larger than this build accepts")]
    TooLarge,
    #[error("the other device did not accept the transfer")]
    Rejected,
}

/// Reject anything that is not six decimal digits before it reaches the PAKE.
pub fn validate_code(code: &str) -> Result<(), PairingError> {
    let ok = code.len() == CODE_LEN && code.bytes().all(|b| b.is_ascii_digit());
    if ok {
        Ok(())
    } else {
        Err(PairingError::BadCode)
    }
}

/// Sender side (PAKE initiator): run the handshake over `stream` under
/// `pairing_code`, then seal `bundle` under the derived channel key and send
/// it. Returns once the receiver has confirmed it opened the sealed bundle —
/// before it has been applied there, which the receiver reports in its own UI.
pub async fn send_bundle<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    pairing_code: &str,
    bundle: &CredentialBundle,
) -> Result<(), PairingError> {
    validate_code(pairing_code)?;

    let (initiator, init_msg) = PakeInitiator::new(pairing_code);
    step(stream.write_all(&init_msg)).await?;

    let mut response = [0u8; RESPONSE_LEN];
    step(stream.read_exact(&mut response)).await?;
    let channel_key = initiator
        .finish(&response)
        .map_err(|_| PairingError::Mismatch)?;

    let sealed = seal::seal_with_key(bundle, &channel_key);
    let mut frame = Vec::with_capacity(4 + sealed.len());
    frame.extend_from_slice(&(sealed.len() as u32).to_be_bytes());
    frame.extend_from_slice(&sealed);
    step(stream.write_all(&frame)).await?;

    // One byte back: the receiver opened the sealed bundle (1) or refused it
    // (anything else, including a hang-up before the byte arrived).
    let mut ack = [0u8; 1];
    step(stream.read_exact(&mut ack)).await?;
    match ack[0] {
        1 => Ok(()),
        _ => Err(PairingError::Rejected),
    }
}

/// Receiver side (PAKE responder): run the handshake over `stream` under
/// `pairing_code`, then read and open the one sealed bundle. The bundle is
/// returned only after the whole exchange has authenticated and decrypted —
/// a caller that applies it can do so unconditionally, and every error return
/// means nothing on this device has been touched.
pub async fn receive_bundle<S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    stream: &mut S,
    pairing_code: &str,
) -> Result<CredentialBundle, PairingError> {
    validate_code(pairing_code)?;

    let mut init_msg = [0u8; KEY_LEN];
    step(stream.read_exact(&mut init_msg)).await?;
    let (responder, response) =
        PakeResponder::new(pairing_code, &init_msg).map_err(|_| PairingError::Mismatch)?;
    step(stream.write_all(&response)).await?;

    let mut len_bytes = [0u8; 4];
    step(stream.read_exact(&mut len_bytes)).await?;
    let len = u32::from_be_bytes(len_bytes);
    if len > MAX_BUNDLE_LEN {
        return Err(PairingError::TooLarge);
    }

    let mut sealed = vec![0u8; len as usize];
    step(stream.read_exact(&mut sealed)).await?;
    let bundle = match seal::open_with_key(&sealed, responder.channel_key()) {
        Ok(bundle) => bundle,
        // Wrong key (codes differed after all) or corrupted bytes. Tell the
        // sender it was refused — best effort, the peer may already be gone —
        // and stop before anything here is touched.
        Err(_) => {
            let _ = stream.write_all(&[0]).await;
            return Err(PairingError::Mismatch);
        }
    };

    // Acknowledge before returning: the sender's success message means "the
    // other device has the bundle", not merely "the bytes left this machine".
    let _ = stream.write_all(&[1]).await;
    Ok(bundle)
}

/// Run one I/O step under the stall timeout. All its failure modes — the
/// deadline passing, the peer hanging up, a mid-frame connection reset — mean
/// the same thing to the session: the other side is not there.
async fn step<T>(
    io: impl std::future::Future<Output = std::io::Result<T>>,
) -> Result<T, PairingError> {
    tokio::time::timeout(STEP_TIMEOUT, io)
        .await
        .map_err(|_| PairingError::Interrupted)?
        .map_err(|_| PairingError::Interrupted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use crate::transfer::BundleAccount;
    use tokio::io::duplex;

    fn sample_bundle(label: &str) -> CredentialBundle {
        let mut bundle = CredentialBundle::default();
        bundle.accounts.insert(
            "openrouter".into(),
            BundleAccount {
                config: ProviderConfig {
                    enabled: true,
                    kind: Some("openrouter".into()),
                    label: Some(label.into()),
                    ..ProviderConfig::default()
                },
                secret: Some("sk-or-v1-abc".into()),
            },
        );
        bundle
    }

    /// Drive one full exchange over a loopback TCP connection — the real
    /// socket seam the desktop adapter will use, not a stand-in.
    #[tokio::test]
    async fn a_matching_code_moves_the_bundle_over_a_real_socket() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let bundle = sample_bundle("Personal");
        let expected = bundle.clone();

        let sender = tokio::spawn(async move {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            send_bundle(&mut stream, "654321", &bundle).await
        });
        let receiver = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            receive_bundle(&mut stream, "654321").await
        });

        assert_eq!(sender.await.unwrap().unwrap(), ());
        assert_eq!(receiver.await.unwrap().unwrap(), expected);
    }

    /// Same exchange over an in-memory duplex: the transport works for any
    /// byte stream, which is what keeps the socket adapter thin. Each half is
    /// spawned owning its end, so a half that finishes early is dropped and
    /// the other side sees a hang-up rather than waiting out the stall
    /// timeout.
    #[tokio::test]
    async fn a_matching_code_moves_the_bundle_over_an_in_process_duplex() {
        let (mut a, mut b) = duplex(64 * 1024);
        let bundle = sample_bundle("Work");
        let expected = bundle.clone();

        let sender = tokio::spawn(async move {
            let sent = send_bundle(&mut a, "000000", &bundle).await;
            drop(a);
            sent
        });
        let receiver = tokio::spawn(async move {
            let received = receive_bundle(&mut b, "000000").await;
            drop(b);
            received
        });

        assert_eq!(sender.await.unwrap().unwrap(), ());
        assert_eq!(receiver.await.unwrap().unwrap(), expected);
    }

    /// A wrong code is one aborted attempt for both sides: the sender's
    /// confirmation check fails, and the receiver — which derived a key the
    /// sender could never compute — sees a hang-up instead of a bundle.
    /// Nothing is returned to apply on either device.
    #[tokio::test]
    async fn mismatched_codes_abort_both_sides_without_a_bundle() {
        let (mut a, mut b) = duplex(64 * 1024);
        let sender = tokio::spawn(async move {
            let result = send_bundle(&mut a, "111111", &sample_bundle("Personal")).await;
            drop(a);
            result
        });
        let receiver = tokio::spawn(async move {
            let result = receive_bundle(&mut b, "222222").await;
            drop(b);
            result
        });

        assert_eq!(sender.await.unwrap().unwrap_err(), PairingError::Mismatch);
        assert_eq!(
            receiver.await.unwrap().unwrap_err(),
            PairingError::Interrupted
        );
    }

    /// An interrupted transfer — the sender vanishes right after the handshake
    /// verified — ends the receiver's session with an error, so a caller that
    /// only applies on `Ok` leaves its configuration untouched.
    #[tokio::test]
    async fn a_sender_that_vanishes_after_the_handshake_never_yields_a_bundle() {
        let (mut a, mut b) = duplex(64 * 1024);
        let sender = tokio::spawn(async move {
            let (initiator, init_msg) = PakeInitiator::new("123456");
            a.write_all(&init_msg).await.unwrap();
            let mut response = [0u8; RESPONSE_LEN];
            a.read_exact(&mut response).await.unwrap();
            // The code matches, then the device walks away mid-transfer.
            initiator.finish(&response).unwrap();
        });
        let receiver = tokio::spawn(async move {
            let result = receive_bundle(&mut b, "123456").await;
            drop(b);
            result
        });

        sender.await.unwrap();
        assert_eq!(
            receiver.await.unwrap().unwrap_err(),
            PairingError::Interrupted
        );
    }

    /// A peer that stalls without hanging up must not wedge the session
    /// forever: the stall timeout ends it. Paused time fires the 30-second
    /// deadline the moment every task is idle.
    #[tokio::test(start_paused = true)]
    async fn a_stalled_peer_ends_the_session_at_the_deadline() {
        let (a, mut b) = duplex(64 * 1024);
        // Connected, then says nothing at all.
        let stall = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            drop(a);
        });

        let received = receive_bundle(&mut b, "123456").await;
        assert_eq!(received.unwrap_err(), PairingError::Interrupted);
        stall.abort();
    }

    /// A length prefix beyond the cap is refused before the payload is read,
    /// so a hostile peer cannot make the receiver allocate arbitrarily.
    #[tokio::test]
    async fn an_oversized_transfer_is_refused_before_any_payload_is_read() {
        let (mut a, mut b) = duplex(64 * 1024);
        let sender = tokio::spawn(async move {
            let (_initiator, init_msg) = PakeInitiator::new("123456");
            a.write_all(&init_msg).await.unwrap();
            let mut response = [0u8; RESPONSE_LEN];
            a.read_exact(&mut response).await.unwrap();
            // Name a bundle far beyond MAX_BUNDLE_LEN, then stop.
            a.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
        });
        let receiver = tokio::spawn(async move {
            let result = receive_bundle(&mut b, "123456").await;
            drop(b);
            result
        });

        sender.await.unwrap();
        assert_eq!(receiver.await.unwrap().unwrap_err(), PairingError::TooLarge);
    }

    /// Bytes that fail to open under the derived key — corrupted in flight,
    /// or a hostile peer's guess at a payload — are refused on the receiver
    /// with nothing returned to apply, and the sender is told the transfer
    /// was refused rather than left assuming it arrived.
    #[tokio::test]
    async fn corrupted_bytes_refuse_the_receiver_and_reject_the_sender() {
        let (mut a, mut b) = duplex(64 * 1024);
        let sender = tokio::spawn(async move {
            // Manual sender so one ciphertext byte can be flipped in flight.
            let (initiator, init_msg) = PakeInitiator::new("123456");
            a.write_all(&init_msg).await.unwrap();
            let mut response = [0u8; RESPONSE_LEN];
            a.read_exact(&mut response).await.unwrap();
            let key = initiator.finish(&response).unwrap();

            let mut sealed = seal::seal_with_key(&sample_bundle("Personal"), &key);
            let last = sealed.len() - 1;
            sealed[last] ^= 0xFF;
            a.write_all(&(sealed.len() as u32).to_be_bytes())
                .await
                .unwrap();
            a.write_all(&sealed).await.unwrap();

            let mut ack = [0u8; 1];
            a.read_exact(&mut ack).await.unwrap();
            ack[0]
        });
        let receiver = tokio::spawn(async move {
            let result = receive_bundle(&mut b, "123456").await;
            drop(b);
            result
        });

        assert_eq!(sender.await.unwrap(), 0);
        assert_eq!(receiver.await.unwrap().unwrap_err(), PairingError::Mismatch);
    }

    /// The sender surfaces an explicit refusal (`0` ack) from the receiver as
    /// its own outcome, distinct from a hang-up.
    #[tokio::test]
    async fn a_receiver_refusal_reaches_the_sender_as_rejected() {
        let (mut a, mut b) = duplex(64 * 1024);
        let receiver = tokio::spawn(async move {
            // Manual receiver: complete the handshake, accept the frame, then
            // refuse it — as the real receiver does when the seal won't open.
            let mut init_msg = [0u8; KEY_LEN];
            b.read_exact(&mut init_msg).await.unwrap();
            let (_responder, response) = PakeResponder::new("123456", &init_msg).unwrap();
            b.write_all(&response).await.unwrap();
            let mut len_bytes = [0u8; 4];
            b.read_exact(&mut len_bytes).await.unwrap();
            let mut sealed = vec![0u8; u32::from_be_bytes(len_bytes) as usize];
            b.read_exact(&mut sealed).await.unwrap();
            b.write_all(&[0]).await.unwrap();
        });

        let sent = send_bundle(&mut a, "123456", &sample_bundle("Personal")).await;
        assert_eq!(sent.unwrap_err(), PairingError::Rejected);
        receiver.await.unwrap();
    }

    /// A code that is not six digits is refused before any I/O: nothing is
    /// written to the stream for the peer to see.
    #[tokio::test]
    async fn a_code_that_is_not_six_digits_is_refused_before_any_io() {
        for bad in ["12345", "1234567", "12a456", "", "abcdef"] {
            let (mut a, mut b) = duplex(64 * 1024);
            let err = send_bundle(&mut a, bad, &sample_bundle("Personal")).await;
            assert_eq!(err, Err(PairingError::BadCode), "code {bad:?}");
            // The stream was never touched: after the sender's end closes, the
            // receiver reads nothing at all — not a half-written handshake.
            drop(a);
            let mut probe = [0u8; 1];
            assert!(b.read_exact(&mut probe).await.is_err());
        }
    }

    #[test]
    fn six_digit_codes_are_accepted_including_leading_zeros() {
        assert_eq!(validate_code("000000"), Ok(()));
        assert_eq!(validate_code("999999"), Ok(()));
    }
}
