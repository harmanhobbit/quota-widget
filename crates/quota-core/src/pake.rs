//! PAKE (Password-Authenticated Key Exchange) handshake for live device
//! pairing over a local network.
//!
//! Two devices that share a short pairing code run this handshake to derive a
//! shared 256-bit channel key. The key is then fed into
//! [`crate::seal::seal_with_key`] / [`crate::seal::open_with_key`] so the
//! credential bundle is encrypted end-to-end with the same format as the file
//! export — the only difference is how the symmetric key was obtained.
//!
//! # Protocol
//!
//! The handshake is a CPace-inspired balanced PAKE over X25519:
//!
//! 1. Both sides hash the pairing code to produce a per-session generator
//!    point `G` on the X25519 curve.
//! 2. Each side picks a random ephemeral scalar, computes its public share
//!    `DH(scalar, G)`, and exchanges it.
//! 3. Each side computes the shared secret `DH(scalar, peer_share)` — and
//!    refuses the handshake outright if it is the all-zero encoding, which is
//!    what a small-order (attacker-chosen) share always produces against a
//!    clamped scalar. Without that contributory check a forged peer could
//!    derive the same channel key without the code and pass step 5.
//! 4. The responder derives the channel key and sends a confirmation tag.
//! 5. The initiator derives the same key and verifies the tag — a mismatch
//!    means the two sides used different pairing codes, or one side sent a
//!    non-contributory share.
//!
//! An attacker who does not know the code cannot compute `G` and therefore
//! cannot solve the DDH problem to recover the key from the public shares
//! alone, and the all-zero check in step 3 closes the one X25519 shortcut
//! (small-order shares) that would have let a forged peer pass without any
//! guess at all. One online guess per connection attempt is the only
//! brute-force path.
//!
//! # Pairing code
//!
//! The pairing code is a **6-digit decimal string** (`"000000"`–`"999999"`),
//! giving 1,000,000 possible codes. Each connection attempt allows exactly one
//! guess — the initiator's `finish()` returns `PakeError::Mismatch` on a wrong
//! code. The application layer must enforce a **single attempt per connection**
//! and **reject repeat connections** with the same code (a code is single-use)
//! so the "one online guess per code" guarantee holds.
//!
//! # Example (in-process, no socket)
//!
//! ```ignore
//! use quota_core::pake::{PakeInitiator, PakeResponder};
//!
//! let (initiator, init_msg) = PakeInitiator::new("123456");
//! let (responder, resp_msg) = PakeResponder::new("123456", &init_msg).unwrap();
//! let init_key = initiator.finish(&resp_msg).unwrap();
//! assert_eq!(init_key, responder.channel_key());
//! ```

use rand::RngCore;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Length of an X25519 public share and the derived channel key.
pub const KEY_LEN: usize = 32;

/// Length of the responder's outbound message: public share (32 bytes) +
/// confirmation tag (32 bytes).
pub const RESPONSE_LEN: usize = 64;

/// Initiator side of the handshake. Created by [`PakeInitiator::new`], which
/// returns the handshake state and the first outbound message.
pub struct PakeInitiator {
    private_scalar: [u8; KEY_LEN],
    public_share: [u8; KEY_LEN],
}

/// Responder side of the handshake. Created by [`PakeResponder::new`], which
/// receives the initiator's message and returns the handshake state plus the
/// response message.
pub struct PakeResponder {
    channel_key: [u8; KEY_LEN],
}

impl PakeInitiator {
    /// Create the initiator side of the handshake, seeded with `pairing_code`.
    /// Returns the initiator state and the 32-byte outbound message to send to
    /// the responder.
    pub fn new(pairing_code: &str) -> (Self, [u8; KEY_LEN]) {
        let generator = derive_generator(pairing_code);

        let mut private_scalar = [0u8; KEY_LEN];
        rand::thread_rng().fill_bytes(&mut private_scalar);
        let clamped_scalar = clamp_scalar(private_scalar);

        let public_share = x25519_dalek::x25519(clamped_scalar, generator);

        (
            PakeInitiator {
                private_scalar: clamped_scalar,
                public_share,
            },
            public_share,
        )
    }

    /// Process the responder's 64-byte response message (public share ||
    /// confirmation tag) and derive the channel key. Returns
    /// [`PakeError::Mismatch`] if the confirmation tag does not verify —
    /// this means the two sides used different pairing codes.
    pub fn finish(self, response: &[u8; RESPONSE_LEN]) -> Result<[u8; KEY_LEN], PakeError> {
        if response.len() != RESPONSE_LEN {
            return Err(PakeError::Mismatch);
        }

        let peer_share: [u8; KEY_LEN] = response[..KEY_LEN]
            .try_into()
            .expect("slice is KEY_LEN bytes");
        let confirmation: [u8; KEY_LEN] = response[KEY_LEN..]
            .try_into()
            .expect("remaining slice is KEY_LEN bytes");

        let shared_secret = shared_secret(self.private_scalar, peer_share)?;
        let channel_key = derive_channel_key(&shared_secret, &self.public_share, &peer_share);
        let expected = derive_confirmation(&channel_key, &self.public_share, &peer_share);

        if constant_time_eq(&confirmation, &expected) {
            Ok(channel_key)
        } else {
            Err(PakeError::Mismatch)
        }
    }
}

impl PakeResponder {
    /// Create the responder side of the handshake, seeded with `pairing_code`
    /// and receiving the initiator's 32-byte outbound message. Returns the
    /// responder state and a 64-byte response message (public share ||
    /// confirmation tag) to send back to the initiator.
    pub fn new(
        pairing_code: &str,
        initiator_msg: &[u8; KEY_LEN],
    ) -> Result<(Self, [u8; RESPONSE_LEN]), PakeError> {
        let generator = derive_generator(pairing_code);

        let mut private_scalar = [0u8; KEY_LEN];
        rand::thread_rng().fill_bytes(&mut private_scalar);
        let clamped_scalar = clamp_scalar(private_scalar);

        let public_share = x25519_dalek::x25519(clamped_scalar, generator);
        let shared_secret = shared_secret(clamped_scalar, *initiator_msg)?;
        let channel_key = derive_channel_key(&shared_secret, initiator_msg, &public_share);
        let confirmation = derive_confirmation(&channel_key, initiator_msg, &public_share);

        let mut response = [0u8; RESPONSE_LEN];
        response[..KEY_LEN].copy_from_slice(&public_share);
        response[KEY_LEN..].copy_from_slice(&confirmation);

        Ok((PakeResponder { channel_key }, response))
    }

    /// The channel key derived by this side of the handshake. Only meaningful
    /// after the initiator's `finish()` has verified the confirmation tag —
    /// until then, the two sides may have used different pairing codes and
    /// produced different keys.
    pub fn channel_key(&self) -> &[u8; KEY_LEN] {
        &self.channel_key
    }
}

/// Hash the pairing code to produce an X25519 generator point. The output is
/// clamped to produce a valid scalar for the key exchange.
fn derive_generator(pairing_code: &str) -> [u8; KEY_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(b"quota-widget-pake-generator-v1");
    hasher.update(pairing_code.as_bytes());
    let hash = hasher.finalize();
    let mut scalar = [0u8; KEY_LEN];
    scalar.copy_from_slice(&hash[..KEY_LEN]);
    clamp_scalar(scalar)
}

/// Multiply our scalar by the peer's share, refusing non-contributory shares.
///
/// X25519 happily accepts public shares that make the output attacker-known:
/// any point of order dividing the cofactor 8 — the "small-order" shares —
/// yields the all-zero secret against a clamped scalar, since clamping makes
/// the scalar a multiple of 8 and 8·P is the identity for such P. A peer (or
/// network attacker) that sends one of those knows the secret the handshake
/// would derive without knowing the pairing code, and could pass the
/// confirmation check with a key of its own choosing — impersonating the
/// other device to steal the bundle, or planting one. The all-zero output is
/// the complete test for this: a legitimate share produces it only with
/// probability ~2⁻²⁵², and no share chosen to zero *our* output can be found
/// without knowing our scalar.
///
/// Both roles run this before any key material is derived, so a refused
/// handshake leaves nothing on either side.
fn shared_secret(
    scalar: [u8; KEY_LEN],
    peer_share: [u8; KEY_LEN],
) -> Result<[u8; KEY_LEN], PakeError> {
    let shared = x25519_dalek::x25519(scalar, peer_share);
    if shared == [0u8; KEY_LEN] {
        return Err(PakeError::Mismatch);
    }
    Ok(shared)
}

/// Derive the channel key from the DH shared secret and the two public shares.
fn derive_channel_key(
    shared_secret: &[u8; KEY_LEN],
    initiator_share: &[u8; KEY_LEN],
    responder_share: &[u8; KEY_LEN],
) -> [u8; KEY_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(b"quota-widget-channel-key-v1");
    hasher.update(shared_secret);
    hasher.update(initiator_share);
    hasher.update(responder_share);
    let hash = hasher.finalize();
    let mut key = [0u8; KEY_LEN];
    key.copy_from_slice(&hash[..KEY_LEN]);
    key
}

/// Derive the confirmation tag the responder sends so the initiator can verify
/// both sides used the same pairing code.
fn derive_confirmation(
    channel_key: &[u8; KEY_LEN],
    initiator_share: &[u8; KEY_LEN],
    responder_share: &[u8; KEY_LEN],
) -> [u8; KEY_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(b"quota-widget-pake-confirm-v1");
    hasher.update(channel_key);
    hasher.update(initiator_share);
    hasher.update(responder_share);
    let hash = hasher.finalize();
    let mut tag = [0u8; KEY_LEN];
    tag.copy_from_slice(&hash[..KEY_LEN]);
    tag
}

/// Clamp a 32-byte scalar for X25519: clear bits 0, 1, 2 of the first byte,
/// clear bit 7 of the last byte, set bit 6 of the last byte.
fn clamp_scalar(mut scalar: [u8; KEY_LEN]) -> [u8; KEY_LEN] {
    scalar[0] &= 248;
    scalar[31] &= 127;
    scalar[31] |= 64;
    scalar
}

/// Constant-time comparison of two byte arrays.
fn constant_time_eq(a: &[u8; KEY_LEN], b: &[u8; KEY_LEN]) -> bool {
    let mut acc = 0u8;
    for i in 0..KEY_LEN {
        acc |= a[i] ^ b[i];
    }
    acc == 0
}

#[derive(Debug, Error, PartialEq)]
pub enum PakeError {
    #[error("pairing codes do not match, or the peer message was tampered with")]
    Mismatch,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use crate::seal;
    use crate::transfer::{BundleAccount, CredentialBundle};

    fn sample_bundle() -> CredentialBundle {
        let mut bundle = CredentialBundle::default();
        bundle.accounts.insert(
            "openrouter".into(),
            BundleAccount {
                config: ProviderConfig {
                    enabled: true,
                    kind: Some("openrouter".into()),
                    label: Some("Personal".into()),
                    ..ProviderConfig::default()
                },
                secret: Some("sk-or-v1-abc".into()),
            },
        );
        bundle
    }

    #[test]
    fn matching_codes_produce_equal_channel_keys() {
        let (initiator, init_msg) = PakeInitiator::new("123456");
        let (responder, resp_msg) = PakeResponder::new("123456", &init_msg).unwrap();
        let init_key = initiator.finish(&resp_msg).unwrap();
        assert_eq!(init_key, *responder.channel_key());
    }

    #[test]
    fn mismatched_codes_fail_to_derive_a_shared_key() {
        let (initiator, init_msg) = PakeInitiator::new("123456");
        let (_responder, resp_msg) = PakeResponder::new("654321", &init_msg).unwrap();
        let err = initiator.finish(&resp_msg).unwrap_err();
        assert_eq!(err, PakeError::Mismatch);
    }

    #[test]
    fn derived_key_round_trips_a_sealed_bundle() {
        let (initiator, init_msg) = PakeInitiator::new("123456");
        let (responder, resp_msg) = PakeResponder::new("123456", &init_msg).unwrap();
        let init_key = initiator.finish(&resp_msg).unwrap();
        assert_eq!(init_key, *responder.channel_key());

        let bundle = sample_bundle();
        let sealed = seal::seal_with_key(&bundle, &init_key);
        let opened = seal::open_with_key(&sealed, &init_key).unwrap();
        assert_eq!(opened, bundle);
    }

    #[test]
    fn different_pairing_codes_never_produce_equal_keys() {
        let (init_a, msg_a) = PakeInitiator::new("000000");
        let (_resp_a, resp_msg_a) = PakeResponder::new("000000", &msg_a).unwrap();
        let key_a = init_a.finish(&resp_msg_a).unwrap();

        let (init_b, msg_b) = PakeInitiator::new("999999");
        let (_resp_b, resp_msg_b) = PakeResponder::new("999999", &msg_b).unwrap();
        let key_b = init_b.finish(&resp_msg_b).unwrap();

        assert_ne!(key_a, key_b);
    }

    #[test]
    fn each_handshake_produces_a_different_key() {
        let (init_a, msg_a) = PakeInitiator::new("123456");
        let (_resp_a, resp_msg_a) = PakeResponder::new("123456", &msg_a).unwrap();
        let key_a = init_a.finish(&resp_msg_a).unwrap();

        let (init_b, msg_b) = PakeInitiator::new("123456");
        let (_resp_b, resp_msg_b) = PakeResponder::new("123456", &msg_b).unwrap();
        let key_b = init_b.finish(&resp_msg_b).unwrap();

        assert_ne!(key_a, key_b);
    }

    #[test]
    fn responder_derives_key_immediately() {
        let (_initiator, init_msg) = PakeInitiator::new("123456");
        let (responder, _resp_msg) = PakeResponder::new("123456", &init_msg).unwrap();
        assert_eq!(responder.channel_key().len(), KEY_LEN);
    }

    /// The X25519 public-share encodings that force the all-zero shared
    /// secret. Every point of order dividing the cofactor 8 does this against
    /// a clamped scalar (clamping makes the scalar a multiple of 8, and 8·P is
    /// the identity for any such P), so a peer sending any of these knows the
    /// secret the handshake would derive. Verified against `x25519_dalek` —
    /// this list is the test fixture, not an input to the check itself, which
    /// is the general all-zero-output test in [`shared_secret`].
    const SMALL_ORDER_SHARES: [[u8; KEY_LEN]; 7] = [
        [0u8; KEY_LEN], // the identity
        {
            let mut one = [0u8; KEY_LEN];
            one[0] = 1;
            one
        },
        hex("e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800"),
        hex("5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157"),
        hex("ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"),
        hex("edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"),
        hex("eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f"),
    ];

    const fn hex(s: &str) -> [u8; KEY_LEN] {
        const fn digit(b: u8) -> u8 {
            match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                _ => panic!("bad hex digit"),
            }
        }
        let bytes = s.as_bytes();
        let mut out = [0u8; KEY_LEN];
        let mut i = 0;
        while i < KEY_LEN {
            out[i] = digit(bytes[i * 2]) * 16 + digit(bytes[i * 2 + 1]);
            i += 1;
        }
        out
    }

    /// S1 regression (review finding): a forged responder that answers a
    /// legitimate initiator with a small-order share must be refused. The
    /// share forces the all-zero shared secret, so an attacker who knows no
    /// code can compute the channel key and a confirmation tag that verifies —
    /// before the contributory check this forged exchange was accepted, and
    /// the sender would have sealed the bundle under an attacker-known key.
    #[test]
    fn a_forged_responder_with_a_small_order_share_is_refused() {
        for share in SMALL_ORDER_SHARES {
            let (initiator, init_msg) = PakeInitiator::new("123456");
            // Everything a network attacker can compute: the forced zero
            // secret, both public shares (theirs is the forged share, ours is
            // in the initiator message), and therefore a tag that verifies.
            let forged_key = derive_channel_key(&[0u8; KEY_LEN], &init_msg, &share);
            let mut forged = [0u8; RESPONSE_LEN];
            forged[..KEY_LEN].copy_from_slice(&share);
            forged[KEY_LEN..].copy_from_slice(&derive_confirmation(&forged_key, &init_msg, &share));

            assert_eq!(
                initiator.finish(&forged),
                Err(PakeError::Mismatch),
                "share {:02x?}",
                &share[..4]
            );
        }
    }

    /// S1 regression: an initiator share drawn from the small-order set must
    /// be refused before the responder derives or exposes anything — the
    /// attacker sending it can derive the channel key without the code and
    /// seal a bundle of their own onto this device.
    #[test]
    fn a_small_order_initiator_share_is_refused() {
        for share in SMALL_ORDER_SHARES {
            assert_eq!(
                PakeResponder::new("123456", &share).err(),
                Some(PakeError::Mismatch),
                "share {:02x?}",
                &share[..4]
            );
        }
    }

    /// A well-formed peer share never trips the contributory check: the
    /// happy path still derives one shared key on both sides.
    #[test]
    fn a_legitimate_share_is_not_mistaken_for_a_small_order_one() {
        let (initiator, init_msg) = PakeInitiator::new("654321");
        let (responder, resp_msg) = PakeResponder::new("654321", &init_msg).unwrap();
        let key = initiator.finish(&resp_msg).unwrap();
        assert_eq!(key, *responder.channel_key());
    }
}
