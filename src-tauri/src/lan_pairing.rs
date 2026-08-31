//! IPC for LAN desktop pairing (issue #154): the live transport for the one
//! credential bundle (ADR-0008).
//!
//! Everything that matters happens in `quota_core` and is already tested
//! there: `transfer` builds and applies the bundle, `pake` authenticates the
//! pairing code (including the contributory-behavior check that refuses
//! small-order shares), `seal` encrypts under the derived channel key, and
//! `pairing` drives the whole exchange over any byte stream with its own
//! stall timeouts and one-attempt-per-connection rules. This file is the thin
//! socket adapter around those: it supplies the bundle's ingredients, binds
//! and dials, and folds a received bundle back into the running config
//! through the same path as the file import.
//!
//! Session rules the adapter owns, per `pairing`'s contract:
//!
//! - **One session per code, either role.** The slot in
//!   [`AppState::lan_pairing`] holds the one armed session — a receive wait
//!   or a send in flight — tagged with a generation so a finishing session
//!   can free only its own slot and never one a later session armed. A
//!   receive session arms exactly one code and accepts exactly one
//!   connection, whatever the outcome; the frontend then clears the code so a
//!   new session needs a fresh one. An attacker therefore gets a single
//!   online guess per code — see `quota_core::pake`.
//! - **Both roles are bounded and cancelable.** A send runs under the same
//!   whole-exchange deadline a receive does, and `lan_pairing_cancel` aborts
//!   the armed session whichever role it is — a peer that dribbles bytes or
//!   goes quiet must not be able to hold the bundle in memory while the user
//!   watches a spinner they cannot leave.
//! - **Cancellation cannot corrupt.** The spawned session is only abortable
//!   while nothing has been received or written: the session disarms itself
//!   (frees its slot) before the apply step, so a Cancel can never land
//!   mid-write.
//!
//! Discovery is deliberately manual — the receiver displays this machine's
//! LAN address for the sender to type — because a zero-configuration
//! discovery protocol would add a dependency and a second attack surface for
//! a one-shot transfer between two devices the user is sitting at.

use crate::credential_transfer::apply_and_commit;
use crate::secrets;
use crate::AppState;
use quota_core::pairing::{self, PairingError};
use quota_core::transfer::{self, ApplyReport};
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

/// Whole-exchange ceiling on top of the per-step stalls, for both roles: a
/// peer that dribbles one byte per stall window could otherwise stretch a
/// 16 MiB transfer indefinitely, and a send that can be stalled indefinitely
/// is a send whose bundle sits in memory with no way out. A real bundle over
/// a real LAN takes seconds. (Tests shrink this so the outer deadline fires
/// before the 30-second per-step deadline.)
#[cfg(not(test))]
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
#[cfg(test)]
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(1);

/// How long dialing may take before "unreachable" is the answer.
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// The one armed pairing session, whichever role it is. At most one at a
/// time: a session's handle lives here from `receive_start`/`send` until the
/// session ends or is cancelled, and the session frees its own slot before
/// its apply step so a Cancel can never land mid-write. A plain `Mutex`
/// because the commands that touch it are short and sync.
///
/// The generation tag exists because a session frees its own slot when it
/// finishes: without the tag, a finishing session could clear a slot a *later*
/// session had armed in between, and that later session would become
/// uncancellable.
#[derive(Default)]
pub struct SessionSlot {
    next_generation: u64,
    armed: Option<(u64, tauri::async_runtime::JoinHandle<()>)>,
}

impl SessionSlot {
    /// Tag a session about to be spawned with the next generation.
    fn reserve(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    /// Arm the session once its task exists, replacing any stale entry.
    fn arm(&mut self, generation: u64, handle: tauri::async_runtime::JoinHandle<()>) {
        self.armed = Some((generation, handle));
    }

    /// A finished session frees its own slot — but only if it is still the
    /// one armed. A cancelled session freed it already, and a newer session
    /// may have armed since.
    fn disarm(&mut self, generation: u64) {
        if self.armed.as_ref().is_some_and(|(g, _)| *g == generation) {
            self.armed = None;
        }
    }

    /// Cancel the armed session, if any. A no-op when nothing is armed —
    /// which is what makes a late Cancel harmless: the session disarms itself
    /// before its apply step, and aborting a finished task does nothing.
    fn cancel(&mut self) {
        if let Some((_, handle)) = self.armed.take() {
            handle.abort();
        }
    }

    /// Whether a session is armed and can be cancelled.
    fn is_armed(&self) -> bool {
        self.armed.is_some()
    }
}

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

/// Reserve a generation for a new session, refusing while one is armed.
/// Sync, so its std MutexGuard never appears in an async body.
fn reserve_session(state: &AppState) -> Result<u64, String> {
    let mut slot = state.lan_pairing.lock().unwrap();
    if slot.is_armed() {
        return Err(
            "A pairing is already in progress on this device — cancel it first.".to_string(),
        );
    }
    Ok(slot.reserve())
}

/// Arm the session once its task exists. Sync, like `reserve_session`.
fn arm_session(state: &AppState, generation: u64, handle: tauri::async_runtime::JoinHandle<()>) {
    state.lan_pairing.lock().unwrap().arm(generation, handle);
}

/// Sender side: build the bundle from this device's accounts, dial the
/// receiver, and run the pairing exchange. The exchange runs in its own task
/// under the session slot, so a stalled or silent receiver is abortable by
/// [`lan_pairing_cancel`] for as long as it lasts, and the whole exchange is
/// bounded by [`EXCHANGE_TIMEOUT`] no matter what the peer does. Returns once
/// the receiver confirmed it opened the sealed bundle; the receiver's own
/// summary is shown there.
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
    let bundle = transfer::build_bundle(&shared, |key| secrets::get(&dir, key));

    let state = state.inner().clone();
    let task_state = state.clone();
    // The slot's std MutexGuard is not Send, so it must never appear in this
    // async body: reserve and arm run inside sync helpers whose guards live
    // and die outside the generator.
    let generation = reserve_session(&state)?;

    // The result travels by channel rather than the task's return value, so
    // that an aborted task — whose result nobody waits for — reads as the
    // cancelled outcome instead of wedging the command forever.
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let code_for_task = code.clone();
    let worker = tauri::async_runtime::spawn(async move {
        let result = send_once(target, &code_for_task, &bundle).await;
        // Whatever the outcome, this session has done its one job; free the
        // slot unless a later session armed since. (On abort this tail never
        // runs — the Cancel command took the handle before aborting.)
        task_state.lan_pairing.lock().unwrap().disarm(generation);
        let _ = result_tx.send(result);
    });
    arm_session(&state, generation, worker);

    result_rx
        .await
        .map_err(|_| "Transfer cancelled — nothing was sent.".to_string())?
}

/// Dial and exchange under one whole-exchange deadline, so no receiver
/// behaviour — hang-up, stall, or a slow dribble that each keeps inside the
/// per-step windows — can hold the sender's session open for long.
async fn send_once(
    target: SocketAddr,
    code: &str,
    bundle: &transfer::CredentialBundle,
) -> Result<(), String> {
    let exchange = async {
        let connect = tokio::net::TcpStream::connect(target);
        let mut stream = tokio::time::timeout(DIAL_TIMEOUT, connect)
            .await
            .map_err(|_| could_not_reach(target))?
            .map_err(|_| could_not_reach(target))?;

        pairing::send_bundle(&mut stream, code, bundle)
            .await
            .map_err(|e| match e {
                PairingError::Mismatch => {
                    "The pairing code did not match. Nothing was transferred.".to_string()
                }
                PairingError::Rejected => {
                    "The other device could not accept the transfer. Nothing was moved.".to_string()
                }
                _ => "The other device hung up or did not respond. Nothing was transferred."
                    .to_string(),
            })
    };
    tokio::time::timeout(EXCHANGE_TIMEOUT, exchange)
        .await
        .map_err(|_| "The transfer stalled — nothing was sent.".to_string())?
}

fn could_not_reach(target: SocketAddr) -> String {
    format!("Could not reach {target} — is the other device waiting to receive?")
}

/// Receiver side, step one: arm `code` and spawn the wait for one connection.
/// The command returns immediately; the session reports through a single
/// `lan-pairing` event carrying either the apply report or the failure.
#[tauri::command]
pub fn lan_pairing_receive_start<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: tauri::State<'_, Arc<AppState>>,
    code: String,
) -> Result<(), String> {
    pairing::validate_code(&code).map_err(|e| e.to_string())?;
    // Bound in the command, where the well-known port is fixed; the tokio
    // wrapper is taken inside the task, which is where a reactor exists.
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, PAIRING_PORT))
        .map_err(|_| {
            format!("The pairing port {PAIRING_PORT} is already in use on this device.")
        })?;
    arm_receive(app, state.inner().clone(), code, listener)
}

/// Arm one receive session over `listener` — the seam shared by the command
/// (which binds the well-known port) and the tests (which bind an ephemeral
/// one so sessions can run in parallel).
fn arm_receive<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: Arc<AppState>,
    code: String,
    listener: std::net::TcpListener,
) -> Result<(), String> {
    // A separate handle for the task, so the slot lock below never borrows
    // across the spawn.
    let task_state = state.clone();
    let generation = reserve_session(&state)?;
    let handle = tauri::async_runtime::spawn(async move {
        let result = receive_session(&task_state, &app, &code, listener, generation).await;
        // The code has done its one job, whatever happened; free the slot so
        // another session can be armed. (On abort this tail never runs — the
        // Cancel command took the handle before aborting — so a stale slot is
        // impossible either way.)
        task_state.lan_pairing.lock().unwrap().disarm(generation);
        let payload = match result {
            Ok(report) => serde_json::json!({ "ok": true, "report": report }),
            Err(error) => serde_json::json!({ "ok": false, "error": error }),
        };
        // The frontend re-reads the imported accounts on `ok`; the `config`
        // event itself was already emitted by the commit inside the session.
        let _ = app.emit("lan-pairing", payload);
    });
    arm_session(&state, generation, handle);
    Ok(())
}

/// Receiver side, step two: the armed session — accept exactly one
/// connection, exchange, and only then commit. Every early return happens
/// before anything on this device is touched.
async fn receive_session<R: tauri::Runtime>(
    state: &Arc<AppState>,
    app: &tauri::AppHandle<R>,
    code: &str,
    listener: std::net::TcpListener,
    generation: u64,
) -> Result<ApplyReport, String> {
    // tokio's `from_std` refuses a socket still in blocking mode; the pair
    // must be flipped before the conversion, or every session dies here.
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let listener = tokio::net::TcpListener::from_std(listener).map_err(|e| e.to_string())?;
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
    state.lan_pairing.lock().unwrap().disarm(generation);
    apply_and_commit(app, state, &bundle).await
}

/// Cancel the armed pairing session, whichever role it is (a receive wait or
/// a send in flight). Aborts only the not-yet-applied part: a session frees
/// its slot before its apply step, so a late Cancel is a no-op rather than a
/// way to interrupt a half-written merge.
#[tauri::command]
pub fn lan_pairing_cancel(state: tauri::State<'_, Arc<AppState>>) {
    state.lan_pairing.lock().unwrap().cancel();
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
    use quota_core::config::{Config, ProviderConfig};
    use quota_core::transfer::{BundleAccount, CredentialBundle};
    use std::collections::HashMap;
    use tauri::{Listener, Manager};

    /// A mock Tauri app over a real `AppState` in a temp dir: enough to run
    /// the pairing commands exactly as the shell would, with real sockets and
    /// a real config/secrets store, but no windows and an ephemeral port.
    fn mock_app() -> (
        tauri::App<tauri::test::MockRuntime>,
        Arc<AppState>,
        tempfile::TempDir,
    ) {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().to_path_buf();
        let config = Config::load(&config_dir).config;
        let state = Arc::new(AppState {
            config_recovery: std::sync::Mutex::new(None),
            lan_pairing: std::sync::Mutex::new(SessionSlot::default()),
            config_dir: config_dir.clone(),
            hide_on_blur: std::sync::atomic::AtomicBool::new(config.hide_on_blur),
            mini_pinned: std::sync::atomic::AtomicBool::new(false),
            reopen_mini_after_popup: std::sync::atomic::AtomicBool::new(false),
            last_drag_ms: std::sync::atomic::AtomicI64::new(0),
            mini_anchor: std::sync::Mutex::new(config.mini_anchor.clone()),
            last_mini_drag_ms: std::sync::atomic::AtomicI64::new(0),
            mini_dragging: std::sync::atomic::AtomicBool::new(false),
            mini_drag_gen: std::sync::atomic::AtomicU64::new(0),
            update: tokio::sync::RwLock::new(None),
            config: tokio::sync::RwLock::new(config),
            snapshots: tokio::sync::RwLock::new(HashMap::new()),
            alert_engine: tokio::sync::Mutex::new(quota_core::alerts::AlertEngine::default()),
            refresh: tokio::sync::Notify::new(),
            oauth_pending: std::sync::Mutex::new(HashMap::new()),
        });
        app.manage(state.clone());
        (app, state, dir)
    }

    /// One pasted-key account, the shape a real transfer moves.
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
                secret: Some("sk-or-v1-test".into()),
            },
        );
        bundle
    }

    /// The `lan-pairing` event payloads, captured through the app's real
    /// event route.
    fn capture_events(
        app: &tauri::App<tauri::test::MockRuntime>,
    ) -> std::sync::mpsc::Receiver<String> {
        let (tx, rx) = std::sync::mpsc::channel();
        app.listen("lan-pairing", move |event| {
            let _ = tx.send(event.payload().to_string());
        });
        rx
    }

    /// The receiver's on-disk state, for the unchanged-on-failure assertions.
    fn disk_state(config_dir: &std::path::Path) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        (
            std::fs::read(config_dir.join("config.json")).ok(),
            std::fs::read(config_dir.join("secrets.json")).ok(),
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_matching_transfer_is_applied_and_reported() {
        let (app, state, _dir) = mock_app();
        let rx = capture_events(&app);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        arm_receive(
            app.handle().clone(),
            state.clone(),
            "123456".into(),
            listener,
        )
        .unwrap();
        assert!(state.lan_pairing.lock().unwrap().is_armed());

        let expected = sample_bundle("Personal");
        let sender = tokio::spawn(async move {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            pairing::send_bundle(&mut stream, "123456", &expected)
                .await
                .unwrap()
        });

        let payload: serde_json::Value =
            serde_json::from_str(&rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap())
                .unwrap();
        assert_eq!(payload["ok"], serde_json::Value::Bool(true));
        // The desktop default config already ships an `openrouter` entry, so
        // the bundle's account merges into it as an update, not an add.
        assert_eq!(
            payload["report"]["accounts"]["openrouter"]["outcome"],
            "updated"
        );
        sender.await.unwrap();

        // The receiver really changed: the account is in the running config
        // and the pasted key landed in its secret store.
        assert!(state
            .config
            .read()
            .await
            .providers
            .contains_key("openrouter"));
        assert_eq!(
            secrets::get(&state.config_dir, "openrouter"),
            Some("sk-or-v1-test".into())
        );
        // Disarmed before the apply, so the slot is free the moment the
        // report arrives.
        assert!(!state.lan_pairing.lock().unwrap().is_armed());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_wrong_code_reports_failure_and_changes_nothing() {
        let (app, state, _dir) = mock_app();
        let rx = capture_events(&app);
        let before = disk_state(&state.config_dir);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        arm_receive(
            app.handle().clone(),
            state.clone(),
            "123456".into(),
            listener,
        )
        .unwrap();

        let sender = tokio::spawn(async move {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            pairing::send_bundle(&mut stream, "999999", &sample_bundle("Personal")).await
        });
        assert_eq!(
            sender.await.unwrap(),
            Err(quota_core::pairing::PairingError::Mismatch)
        );

        let payload: serde_json::Value =
            serde_json::from_str(&rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap())
                .unwrap();
        assert_eq!(payload["ok"], serde_json::Value::Bool(false));
        // The receiver never learns the code was wrong — it derived a key
        // the sender could never compute, so all it sees is the sender
        // hanging up mid-frame (the protocol's designed wrong-code shape).
        // Either way: nothing was changed.
        assert!(payload["error"]
            .as_str()
            .unwrap()
            .contains("Nothing was changed"));

        // The receiver is exactly as it was, and the failed session still
        // freed the slot for the next one.
        assert_eq!(before, disk_state(&state.config_dir));
        assert!(!state.lan_pairing.lock().unwrap().is_armed());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_a_waiting_session_ends_it_without_a_trace() {
        let (app, state, _dir) = mock_app();
        let rx = capture_events(&app);
        let before = disk_state(&state.config_dir);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        arm_receive(
            app.handle().clone(),
            state.clone(),
            "123456".into(),
            listener,
        )
        .unwrap();
        assert!(state.lan_pairing.lock().unwrap().is_armed());

        lan_pairing_cancel(app.state::<Arc<AppState>>().clone());
        assert!(!state.lan_pairing.lock().unwrap().is_armed());

        // The listener died with the aborted task: the address stops
        // answering within a moment (bounded, not a poll-forever loop).
        let mut refused = false;
        for _ in 0..50 {
            match tokio::time::timeout(
                std::time::Duration::from_millis(100),
                tokio::net::TcpStream::connect(addr),
            )
            .await
            {
                Ok(Ok(_)) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
                _ => {
                    refused = true;
                    break;
                }
            }
        }
        assert!(
            refused,
            "the cancelled session's listener is still answering"
        );

        // No outcome was reported and nothing was touched.
        assert!(rx
            .recv_timeout(std::time::Duration::from_millis(200))
            .is_err());
        assert_eq!(before, disk_state(&state.config_dir));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancel_after_a_completed_transfer_is_a_noop_and_a_new_session_can_arm() {
        let (app, state, _dir) = mock_app();
        let rx = capture_events(&app);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        arm_receive(
            app.handle().clone(),
            state.clone(),
            "123456".into(),
            listener,
        )
        .unwrap();
        let sender = tokio::spawn(async move {
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            pairing::send_bundle(&mut stream, "123456", &sample_bundle("Personal"))
                .await
                .unwrap()
        });
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        sender.await.unwrap();

        // The session disarmed itself before applying, so a Cancel after the
        // report can neither undo nor corrupt the applied account.
        assert!(!state.lan_pairing.lock().unwrap().is_armed());
        lan_pairing_cancel(app.state::<Arc<AppState>>().clone());
        assert!(state
            .config
            .read()
            .await
            .providers
            .contains_key("openrouter"));

        // The freed slot accepts the next session immediately.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert!(arm_receive(
            app.handle().clone(),
            state.clone(),
            "654321".into(),
            listener
        )
        .is_ok());
        lan_pairing_cancel(app.state::<Arc<AppState>>().clone());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_second_session_is_refused_while_one_is_armed() {
        let (app, state, _dir) = mock_app();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        arm_receive(
            app.handle().clone(),
            state.clone(),
            "123456".into(),
            listener,
        )
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let err = arm_receive(
            app.handle().clone(),
            state.clone(),
            "654321".into(),
            listener,
        )
        .unwrap_err();
        assert!(err.contains("already in progress"), "{err}");
        lan_pairing_cancel(app.state::<Arc<AppState>>().clone());
    }

    /// S2: a receiver that accepts and then goes silent ends the send at the
    /// whole-exchange deadline — the sender is never held open indefinitely.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stalled_receiver_ends_the_send_at_the_whole_exchange_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let silent = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });

        let result = send_once(addr, "123456", &sample_bundle("Personal")).await;
        assert!(result.unwrap_err().contains("stalled"));
        silent.abort();
    }

    /// S2: cancelling a send in flight stops it, reports the cancelled
    /// outcome to the waiting command, and frees the slot.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_a_send_stops_it_and_frees_the_slot() {
        let (app, state, _dir) = mock_app();
        // A real account so the bundle exists and is worth holding.
        state.config.write().await.providers.insert(
            "openrouter".into(),
            ProviderConfig {
                enabled: true,
                kind: Some("openrouter".into()),
                label: Some("Mine".into()),
                ..ProviderConfig::default()
            },
        );
        secrets::set(&state.config_dir, "openrouter", "sk-or-v1-mine").unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let silent = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });

        let cmd = lan_pairing_send(
            app.state::<Arc<AppState>>().clone(),
            "123456".into(),
            addr.to_string(),
        );
        tokio::pin!(cmd);
        // Drive the command by hand until the session is armed, so the cancel
        // below lands while the send is genuinely in flight.
        let mut armed = false;
        for _ in 0..200 {
            if state.lan_pairing.lock().unwrap().is_armed() {
                armed = true;
                break;
            }
            if let std::task::Poll::Ready(r) = futures_util::poll!(cmd.as_mut()) {
                panic!("the send finished before it could be cancelled: {r:?}");
            }
            tokio::task::yield_now().await;
        }
        assert!(armed, "the send never armed the session slot");

        lan_pairing_cancel(app.state::<Arc<AppState>>().clone());
        assert_eq!(
            cmd.await,
            Err("Transfer cancelled — nothing was sent.".to_string())
        );
        assert!(!state.lan_pairing.lock().unwrap().is_armed());
        silent.abort();
    }

    #[test]
    fn disarm_only_frees_its_own_generation() {
        let mut slot = SessionSlot::default();
        let g1 = slot.reserve();
        let g2 = slot.reserve();
        assert_ne!(g1, g2);
        // A session that finishes after a later one armed must not free the
        // later session's slot.
        slot.armed = Some((g2, tauri::async_runtime::spawn(async {})));
        slot.disarm(g1);
        assert!(slot.is_armed());
        slot.disarm(g2);
        assert!(!slot.is_armed());
    }

    #[test]
    fn a_cancelled_slot_is_free_and_a_second_cancel_is_a_noop() {
        let mut slot = SessionSlot::default();
        let g = slot.reserve();
        slot.armed = Some((g, tauri::async_runtime::spawn(async {})));
        slot.cancel();
        assert!(!slot.is_armed());
        slot.cancel();
        assert!(!slot.is_armed());
    }

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
