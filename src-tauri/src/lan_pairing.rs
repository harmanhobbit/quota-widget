//! IPC for LAN desktop pairing (issue #154): the live transport for the one
//! credential bundle (ADR-0008).
//!
//! Everything that matters happens in `quota_core` and is already tested
//! there: `transfer` builds and applies the bundle, `pake` authenticates the
//! pairing code, `seal` encrypts under the derived channel key, and
//! `pairing` drives the whole exchange over any byte stream with its own
//! stall timeouts and one-attempt-per-connection rules. This file is the thin
//! socket adapter around those: it supplies the bundle's ingredients, binds
//! and dials, and folds a received bundle back into the running config
//! through the same path as the file import.
//!
//! Session rules the adapter owns, per `pairing`'s contract:
//!
//! - **One session per code.** A receive session arms exactly one code and
//!   accepts exactly one connection, whatever the outcome; the frontend then
//!   clears the code so a new session needs a fresh one. An attacker
//!   therefore gets a single online guess per code — see `quota_core::pake`.
//! - **Cancellation cannot corrupt.** The spawned session is only abortable
//!   while nothing has been received or written: the task disarms itself
//!   (drops its handle) before the apply step runs, so a Cancel press can
//!   never land mid-apply.
//!
//! Discovery is deliberately manual — the receiver displays this machine's
//! LAN address for the sender to type — because a zero-configuration
//! discovery protocol would add a dependency and a second attack surface for
//! a one-shot transfer between two devices the user is sitting at.

use crate::credential_transfer::apply_and_commit;
use crate::secrets;
use crate::AppState;
use quota_core::pairing::{self, PairingError};
use quota_core::transfer::ApplyReport;
use rand::Rng;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tauri::Emitter;

/// The fixed port the receiver listens on. The sender only has to type the
/// address; a well-known port is what makes that possible. Unassigned by
/// IANA, like the rest of this app's own choices.
pub const PAIRING_PORT: u16 = 45454;

/// How long a receiver waits, once armed, for the sender to connect. Bounds
/// the window in which this device answers pairing attempts, so an armed code
/// is never left listening overnight. The exchange that follows has its own
/// shorter per-step timeout inside `quota_core::pairing`.
const LISTEN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Whole-exchange ceiling on top of the per-step stalls: a peer that dribbles
/// one byte per stall window could otherwise stretch a 16 MiB transfer
/// indefinitely. A real bundle over a real LAN takes seconds.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How long dialing may take before "unreachable" is the answer.
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// This machine's LAN address(es), best effort, for the receiver to show the
/// sender. The UDP socket sends nothing; connecting it just makes the kernel
/// pick the interface a default route would use, which on a single-LAN
/// desktop is the address the other device can reach.
#[tauri::command]
pub fn lan_pairing_address() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(sock) = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0)) {
        if sock.connect(("8.8.8.8", 80)).is_ok() {
            if let Ok(local) = sock.local_addr() {
                if let std::net::IpAddr::V4(v4) = local.ip() {
                    if !v4.is_loopback() && !v4.is_unspecified() {
                        out.push(v4.to_string());
                    }
                }
            }
        }
    }
    out
}

/// Parse the address the sender typed. A bare IP gets the well-known port;
/// an explicit `ip:port` is taken as-is (both IPv4-only, which is all
/// `lan_pairing_address` advertises).
fn parse_target(address: &str) -> Option<SocketAddr> {
    let address = address.trim();
    if address.contains(':') {
        address.parse().ok()
    } else {
        format!("{address}:{PAIRING_PORT}").parse().ok()
    }
}

/// Sender side: build the bundle from this device's accounts, dial the
/// receiver, and run the pairing exchange. Returns once the receiver
/// confirmed it opened the sealed bundle; the receiver's own summary is
/// shown there.
#[tauri::command]
pub async fn lan_pairing_send(
    state: tauri::State<'_, Arc<AppState>>,
    code: String,
    address: String,
) -> Result<(), String> {
    pairing::validate_code(&code).map_err(|e| e.to_string())?;
    let target = parse_target(&address).ok_or_else(|| {
        format!("'{address}' is not an IPv4 address — use the one the other device is showing")
    })?;

    let cfg = state.config.read().await.clone();
    let (shared, _prefs) = cfg.split();
    let dir = state.config_dir.clone();
    let bundle = quota_core::transfer::build_bundle(&shared, |key| secrets::get(&dir, key));

    let connect = tokio::net::TcpStream::connect(target);
    let mut stream = tokio::time::timeout(DIAL_TIMEOUT, connect)
        .await
        .map_err(|_| format!("Could not reach {target} — is the other device waiting to receive?"))?
        .map_err(|_| {
            format!("Could not reach {target} — is the other device waiting to receive?")
        })?;

    pairing::send_bundle(&mut stream, &code, &bundle)
        .await
        .map_err(|e| match e {
            PairingError::Mismatch => {
                "The pairing code did not match. Nothing was transferred.".to_string()
            }
            PairingError::Rejected => {
                "The other device could not accept the transfer. Nothing was moved.".to_string()
            }
            _ => {
                "The other device hung up or did not respond. Nothing was transferred.".to_string()
            }
        })
}

/// Receiver side, step one: arm `code` and spawn the wait for one connection.
/// The command returns immediately; the session reports through a single
/// `lan-pairing` event carrying either the apply report or the failure.
#[tauri::command]
pub fn lan_pairing_receive_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    code: String,
) -> Result<(), String> {
    pairing::validate_code(&code).map_err(|e| e.to_string())?;
    let mut session = state.lan_pairing.lock().unwrap();
    if session.is_some() {
        return Err("A pairing is already waiting on this device — cancel it first.".to_string());
    }
    let state = state.inner().clone();
    let handle = tauri::async_runtime::spawn(async move {
        let result = receive_session(&state, &app, &code).await;
        // The code has done its one job, whatever happened; free the slot so
        // another session can be armed. (On abort this tail never runs — the
        // Cancel command took the handle before aborting — so a stale slot is
        // impossible either way.)
        *state.lan_pairing.lock().unwrap() = None;
        let payload = match result {
            Ok(report) => serde_json::json!({ "ok": true, "report": report }),
            Err(error) => serde_json::json!({ "ok": false, "error": error }),
        };
        // The frontend re-reads the imported accounts on `ok`; the `config`
        // event itself was already emitted by the commit inside the session.
        let _ = app.emit("lan-pairing", payload);
    });
    *session = Some(handle);
    Ok(())
}

/// Receiver side, step two: the armed session — bind, accept exactly one
/// connection, exchange, and only then commit. Every early return happens
/// before anything on this device is touched.
async fn receive_session(
    state: &Arc<AppState>,
    app: &tauri::AppHandle,
    code: &str,
) -> Result<ApplyReport, String> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, PAIRING_PORT))
        .await
        .map_err(|_| {
            format!("The pairing port {PAIRING_PORT} is already in use on this device.")
        })?;
    let accepted = tokio::time::timeout(LISTEN_TIMEOUT, listener.accept()).await;
    // One connection per armed code: stop listening before the exchange, so
    // nothing else can connect under it while work is in progress.
    drop(listener);
    let (mut stream, _peer) = accepted
        .map_err(|_| "Nobody connected within five minutes — pairing cancelled.".to_string())?
        .map_err(|_| "A connection arrived but could not be used.".to_string())?;

    let bundle = tokio::time::timeout(EXCHANGE_TIMEOUT, pairing::receive_bundle(&mut stream, code))
        .await
        .map_err(|_| "The transfer stalled — nothing was changed.".to_string())?
        .map_err(|e| format!("{e} Nothing was changed."))?;

    // Past this point the exchange has succeeded, so the session disarms
    // itself: from here on a Cancel is a no-op rather than a way to interrupt
    // a half-written merge.
    *state.lan_pairing.lock().unwrap() = None;
    apply_and_commit(app, state, &bundle).await
}

/// Cancel an armed receive session. Aborts only the not-yet-applied part: the
/// session disarms itself before apply, so a late Cancel is a no-op rather
/// than a way to interrupt a half-written merge.
#[tauri::command]
pub fn lan_pairing_receive_cancel(state: tauri::State<'_, Arc<AppState>>) {
    if let Some(handle) = state.lan_pairing.lock().unwrap().take() {
        handle.abort();
    }
}

/// A fresh 6-digit code for a receive session, shown for the user to type on
/// the sending device. Generated here — not in the webview — so the value the
/// receiver arms is the one actually displayed, and so codes are drawn
/// uniformly rather than by whatever `Math.random` happens to produce.
#[tauri::command]
pub fn lan_pairing_generate_code() -> String {
    let mut rng = rand::thread_rng();
    (0..6)
        .map(|_| char::from(b'0' + rng.gen_range(0..10u8)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_ip_gets_the_well_known_port() {
        assert_eq!(
            parse_target("192.168.1.20"),
            Some(SocketAddr::from(([192, 168, 1, 20], PAIRING_PORT)))
        );
    }

    #[test]
    fn an_explicit_port_is_taken_as_given() {
        assert_eq!(
            parse_target("192.168.1.20:5000"),
            Some(SocketAddr::from(([192, 168, 1, 20], 5000)))
        );
    }

    #[test]
    fn whitespace_is_tolerated_but_garbage_is_not() {
        assert_eq!(parse_target(" 10.0.0.2 "), parse_target("10.0.0.2"));
        assert_eq!(parse_target("not an address"), None);
        assert_eq!(parse_target(""), None);
    }
}
