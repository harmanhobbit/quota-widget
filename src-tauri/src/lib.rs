mod codex_oauth;
mod oauth;
mod poller;
mod secrets;
mod tray;
#[cfg(target_os = "linux")]
mod tray_linux;
mod updates;

use quota_core::alerts::AlertEngine;
use quota_core::config::Config;
use quota_core::model::UsageSnapshot;
use quota_core::providers::{providers_for, ProviderCtx};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager, WindowEvent};
use tauri_plugin_autostart::ManagerExt;
use tokio::sync::{Mutex, Notify, RwLock};

pub struct AppState {
    pub config_dir: PathBuf,
    pub config: RwLock<Config>,
    pub snapshots: RwLock<HashMap<String, UsageSnapshot>>,
    pub alert_engine: Mutex<AlertEngine>,
    /// Poked to trigger an immediate poll cycle.
    pub refresh: Notify,
    /// In-flight built-in Claude sign-in (PKCE verifier + state).
    pub oauth_pending: std::sync::Mutex<HashMap<String, oauth::PendingLogin>>,
    /// Mirror of config.hide_on_blur, readable from the sync event loop.
    pub hide_on_blur: std::sync::atomic::AtomicBool,
    /// Pinning belongs only to the compact tray-click summary and lasts for
    /// this process, never in the user's config file.
    pub mini_pinned: std::sync::atomic::AtomicBool,
    /// A pinned mini is temporarily hidden while the full popup is open. It
    /// returns when that interrupting popup closes, without turning pinning
    /// into a persistent config setting.
    pub reopen_mini_after_popup: std::sync::atomic::AtomicBool,
    /// Millis timestamp of the last title-bar press — blur events within the
    /// grace window are drag-induced (tauri#10767), not click-away.
    pub last_drag_ms: std::sync::atomic::AtomicI64,
    /// Mirror of config.mini_anchor, readable from the sync event loop where
    /// the async config lock cannot be taken.
    pub mini_anchor: std::sync::Mutex<quota_core::config::MiniAnchor>,
    /// Millis timestamp of the last mini-summary title-bar press. The summary
    /// is click-away transient, so without a grace period the focus loss a
    /// native drag causes (tauri#10767) would dismiss it as it was being moved.
    pub last_mini_drag_ms: std::sync::atomic::AtomicI64,
    /// Set while the mini summary's title bar is held down, so the move events
    /// that follow are known to be the user dragging rather than our own
    /// anchoring. Cleared when the drag is resolved into an anchor.
    pub mini_dragging: std::sync::atomic::AtomicBool,
    /// Bumped by every move of a dragging mini summary. A native drag has no
    /// end event — the OS owns the pointer, so no mouseup reaches the webview —
    /// so the drop is detected by movement going quiet, and this counter is how
    /// a settle timer knows nothing moved while it waited.
    pub mini_drag_gen: std::sync::atomic::AtomicU64,
    /// Last known upstream release, or `None` when no check has succeeded, the
    /// build is a dev branch, or the newest release is not newer than this one.
    pub update: RwLock<Option<quota_core::update::UpdateInfo>>,
}

/// The frontend's first request needs both pieces of startup state. Keeping
/// them on the established snapshot IPC route avoids a second request that
/// could leave Settings waiting indefinitely.
#[derive(serde::Serialize)]
struct InitialState {
    snapshots: Vec<UsageSnapshot>,
    config: Config,
}

impl AppState {
    fn provider_ctx(&self, config: Config) -> ProviderCtx {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let secrets = secrets::load_all(&self.config_dir, &config);
        let mut ctx = ProviderCtx::new(home, self.config_dir.clone(), secrets, config);
        // Adapters that rotate tokens (Claude OAuth refresh) persist them here.
        let dir = self.config_dir.clone();
        ctx.on_secret_update = Some(std::sync::Arc::new(move |key: &str, value: &str| {
            if let Err(e) = secrets::set(&dir, key, value) {
                eprintln!("failed to persist rotated secret {key}: {e}");
            }
        }));
        ctx
    }
}

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("quota-widget")
}

// ---- IPC commands -----------------------------------------------------------

#[tauri::command]
async fn get_snapshots(state: tauri::State<'_, Arc<AppState>>) -> Result<InitialState, String> {
    let map = state.snapshots.read().await;
    let cfg = state.config.read().await;
    // Configured display order, enabled providers only. `providers_for()`
    // iterates the ordered config map, while the HashMap is lookup-only.
    let mut out = Vec::new();
    for p in providers_for(&cfg) {
        if cfg
            .providers
            .get(p.id())
            .map(|c| c.enabled)
            .unwrap_or(false)
        {
            if let Some(s) = map.get(p.id()) {
                out.push(s.clone());
            }
        }
    }
    // The configured display order, which may be a usage/expiry sort rather
    // than the hand-arranged config order.
    cfg.sort_snapshots(&mut out);
    Ok(InitialState {
        snapshots: out,
        config: cfg.clone(),
    })
}

/// Cargo supplies this from the workspace's single version source at build
/// time, so the Settings footer can never drift from the packaged app.
#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
async fn set_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    config: Config,
) -> Result<(), String> {
    config.save(&state.config_dir).map_err(|e| e.to_string())?;
    state
        .hide_on_blur
        .store(config.hide_on_blur, std::sync::atomic::Ordering::Relaxed);
    *state.mini_anchor.lock().unwrap() = config.mini_anchor.clone();
    *state.config.write().await = config.clone();
    // A monitor chosen in Settings should move a visible summary now, not on
    // its next showing — the picker is otherwise a control with no visible
    // effect. Corner and monitor both come from the anchor, so this covers
    // either being changed.
    if let Some(mini) = app.get_webview_window("mini") {
        if mini.is_visible().unwrap_or(false) {
            tray::anchor_to(&mini, &config.mini_anchor);
        }
    }
    let _ = app.emit("config", &config);
    // Apply autostart immediately.
    let autolaunch = app.autolaunch();
    let result = if config.autostart {
        autolaunch.enable()
    } else {
        autolaunch.disable()
    };
    if let Err(e) = result {
        eprintln!("autostart: {e}"); // non-fatal (e.g. unsupported desktop)
    }
    state.refresh.notify_one();
    Ok(())
}

#[tauri::command]
fn set_secret(
    state: tauri::State<'_, Arc<AppState>>,
    provider: String,
    value: String,
) -> Result<(), String> {
    secrets::set(&state.config_dir, &provider, &value)
}

#[tauri::command]
fn has_secret(state: tauri::State<'_, Arc<AppState>>, provider: String) -> bool {
    secrets::get(&state.config_dir, &provider)
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}

#[tauri::command]
fn clear_secret(state: tauri::State<'_, Arc<AppState>>, provider: String) -> Result<(), String> {
    secrets::clear(&state.config_dir, &provider)
}

#[tauri::command]
async fn refresh_now(state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    state.refresh.notify_one();
    Ok(())
}

/// One-off fetch for the settings page's "Test" button. Ignores the enabled
/// flag so a provider can be verified before switching it on.
#[tauri::command]
async fn test_provider(
    state: tauri::State<'_, Arc<AppState>>,
    provider: String,
) -> Result<UsageSnapshot, String> {
    let cfg = state.config.read().await.clone();
    let ctx = state.provider_ctx(cfg.clone());
    for p in providers_for(&cfg) {
        if p.id() == provider {
            return Ok(match p.fetch(&ctx).await {
                Ok(s) => s,
                Err(e) => UsageSnapshot::failed(p.id(), p.name(), e),
            });
        }
    }
    Err(format!("unknown provider: {provider}"))
}

/// Begin the built-in Claude sign-in: opens the browser and returns the URL
/// (shown in the UI as a copyable fallback).
#[tauri::command]
fn claude_oauth_start(
    state: tauri::State<'_, Arc<AppState>>,
    provider: String,
) -> Result<String, String> {
    let (url, pending) = oauth::start();
    state
        .oauth_pending
        .lock()
        .unwrap()
        .insert(provider, pending);
    if let Err(e) = tauri_plugin_opener::open_url(&url, None::<&str>) {
        eprintln!("browser open failed: {e}"); // URL is still shown in the UI
    }
    Ok(url)
}

/// Complete the sign-in with the pasted `code#state` string.
#[tauri::command]
async fn claude_oauth_finish(
    state: tauri::State<'_, Arc<AppState>>,
    code: String,
    provider: String,
) -> Result<(), String> {
    let pending = state
        .oauth_pending
        .lock()
        .unwrap()
        .remove(&provider)
        .ok_or_else(|| "no sign-in in progress — click Sign in first".to_string())?;
    let http = reqwest::Client::new();
    let tokens = oauth::finish(&http, &pending, &code).await?;
    secrets::set(
        &state.config_dir,
        &secrets::oauth_key(&provider),
        &tokens.to_secret_json(),
    )?;
    state.refresh.notify_one();
    Ok(())
}

/// Begin the built-in Codex sign-in. Unlike Claude's paste-back flow, the user
/// types a short code into the browser and we poll — so this returns the code
/// for display and spawns the wait in the background, emitting `codex-oauth`
/// when it resolves.
#[tauri::command]
async fn codex_oauth_start(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    provider: String,
) -> Result<serde_json::Value, String> {
    let http = reqwest::Client::new();
    let login = codex_oauth::start(&http).await?;
    let shown = serde_json::json!({
        "user_code": login.user_code,
        "verification_url": login.verification_url,
    });
    if let Err(e) = tauri_plugin_opener::open_url(&login.verification_url, None::<&str>) {
        eprintln!("browser open failed: {e}"); // URL is still shown in the UI
    }

    // Polling blocks for up to 15 minutes; don't hold the IPC call open.
    let state = (*state).clone();
    tauri::async_runtime::spawn(async move {
        let payload = match codex_oauth::poll_for_tokens(&http, &login).await {
            Ok(tokens) => {
                match secrets::set(
                    &state.config_dir,
                    &secrets::oauth_key(&provider),
                    &tokens.to_string(),
                ) {
                    Ok(()) => {
                        state.refresh.notify_one();
                        serde_json::json!({ "ok": true, "provider": provider.clone() })
                    }
                    Err(e) => {
                        serde_json::json!({ "ok": false, "error": e, "provider": provider.clone() })
                    }
                }
            }
            Err(e) => serde_json::json!({ "ok": false, "error": e, "provider": provider.clone() }),
        };
        let _ = app.emit("codex-oauth", payload);
    });

    Ok(shown)
}

/// True when running as a native Wayland client, where always-on-top has no
/// protocol (xdg-shell lacks it; tao#1134) and the popup sinks behind other
/// windows. XWayland reports as x11 here and behaves correctly, so this is
/// specifically "the broken case", not merely "on Linux".
#[tauri::command]
fn on_wayland() -> bool {
    if std::env::var("GDK_BACKEND").is_ok_and(|v| v.eq_ignore_ascii_case("x11")) {
        return false;
    }
    std::env::var("WAYLAND_DISPLAY").is_ok_and(|v| !v.is_empty())
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland"))
}

#[tauri::command]
fn hide_window(window: tauri::Window) {
    // The mini summary's close button must also drop the pin, so it takes the
    // shared path rather than hiding itself directly.
    if window.label() == "mini" {
        use tauri::Manager;
        tray::hide_mini(window.app_handle());
        return;
    }
    tray::hide_popup(window.app_handle());
}

#[tauri::command]
fn set_mini_pinned(
    state: tauri::State<'_, Arc<AppState>>,
    window: tauri::WebviewWindow,
    pinned: bool,
) {
    if window.label() != "mini" {
        return;
    }
    state
        .mini_pinned
        .store(pinned, std::sync::atomic::Ordering::Relaxed);
    let _ = window.set_always_on_top(pinned);
    if pinned {
        let anchor = state.mini_anchor.lock().unwrap().clone();
        tray::anchor_to(&window, &anchor);
    }
}

/// Fit the mini summary's height to its rendered content.
///
/// The main window does this from JS with `setSize`, but `mini`'s capability
/// deliberately withholds sizing and positioning permissions — Rust owns both —
/// so the frontend reports a measurement and Rust performs the move. Guarded on
/// the label so nothing else can drive it. (`start-dragging` is the one
/// window permission `mini` does hold: a native drag can only begin from the
/// webview's own pointer press, so it cannot be driven from here.)
#[tauri::command]
fn set_mini_height(app: tauri::AppHandle, window: tauri::WebviewWindow, height: f64) {
    if window.label() != "mini" {
        return;
    }
    tray::resize_mini_to(&window, &tray::current_anchor(&app), height);
}

/// Fired by the frontend on a title-bar mousedown, just before the native
/// drag begins — arms the blur grace period.
#[tauri::command]
fn note_drag(state: tauri::State<'_, Arc<AppState>>) {
    state.last_drag_ms.store(
        chrono::Utc::now().timestamp_millis(),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// A connected monitor, as the Settings picker needs to describe it. Tauri
/// exposes no friendlier identity than the name (`DP-1`, `\\.\DISPLAY1`), so
/// the resolution and left-to-right order are what make a row recognisable.
#[derive(serde::Serialize)]
struct MonitorInfo {
    /// `None` for a monitor the platform did not name; such a monitor cannot be
    /// pinned, because there would be nothing to store. See ADR 0001.
    name: Option<String>,
    width: u32,
    height: u32,
    /// Virtual-desktop x, so the frontend can order the list left to right.
    x: i32,
    primary: bool,
}

#[tauri::command]
fn list_monitors(window: tauri::WebviewWindow) -> Vec<MonitorInfo> {
    let primary = window
        .primary_monitor()
        .ok()
        .flatten()
        .and_then(|m| m.name().cloned());
    let mut monitors: Vec<MonitorInfo> = window
        .available_monitors()
        .unwrap_or_default()
        .into_iter()
        .map(|m| MonitorInfo {
            primary: primary.is_some() && m.name() == primary.as_ref(),
            name: m.name().cloned(),
            width: m.size().width,
            height: m.size().height,
            x: m.position().x,
        })
        .collect();
    monitors.sort_by_key(|m| m.x);
    monitors
}

/// Fired by the mini summary on a title-bar mousedown, just before the native
/// drag begins. Arms the move handler so the moves that follow are attributed
/// to the user rather than to our own anchoring and resizing, which also call
/// `set_position` and must never rewrite the user's choice.
#[tauri::command]
fn note_mini_drag(state: tauri::State<'_, Arc<AppState>>) {
    use std::sync::atomic::Ordering::Relaxed;
    state.mini_dragging.store(true, Relaxed);
    // Also arms the blur grace period: the summary is click-away transient, and
    // the focus loss a native drag causes must not dismiss it mid-move.
    state
        .last_mini_drag_ms
        .store(chrono::Utc::now().timestamp_millis(), Relaxed);
}

#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

// ---- app entry --------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let config_dir = config_dir();
    let config = Config::load(&config_dir);
    let state = Arc::new(AppState {
        config_dir,
        hide_on_blur: std::sync::atomic::AtomicBool::new(config.hide_on_blur),
        mini_pinned: std::sync::atomic::AtomicBool::new(false),
        reopen_mini_after_popup: std::sync::atomic::AtomicBool::new(false),
        last_drag_ms: std::sync::atomic::AtomicI64::new(0),
        mini_anchor: std::sync::Mutex::new(config.mini_anchor.clone()),
        last_mini_drag_ms: std::sync::atomic::AtomicI64::new(0),
        mini_dragging: std::sync::atomic::AtomicBool::new(false),
        mini_drag_gen: std::sync::atomic::AtomicU64::new(0),
        update: RwLock::new(None),
        config: RwLock::new(config),
        snapshots: RwLock::new(HashMap::new()),
        alert_engine: Mutex::new(AlertEngine::default()),
        refresh: Notify::new(),
        oauth_pending: std::sync::Mutex::new(HashMap::new()),
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Relaunching the exe focuses the existing popup.
            tray::show_popup(app, None);
        }))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        // Desktop-only: the crate is not built for mobile targets, and there is
        // no in-place install there to expose.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(state.clone())
        .invoke_handler(tauri::generate_handler![
            get_snapshots,
            app_version,
            set_config,
            set_secret,
            has_secret,
            clear_secret,
            refresh_now,
            test_provider,
            claude_oauth_start,
            claude_oauth_finish,
            codex_oauth_start,
            on_wayland,
            hide_window,
            set_mini_pinned,
            set_mini_height,
            note_drag,
            note_mini_drag,
            list_monitors,
            updates::update_status,
            updates::check_update_now,
            quit,
        ])
        .setup(move |app| {
            #[cfg(target_os = "linux")]
            tray_linux::create_tray(app.handle().clone(), state.clone());
            #[cfg(not(target_os = "linux"))]
            tray::create_tray(app.handle())?;
            poller::spawn(app.handle().clone(), state.clone());
            updates::spawn(app.handle().clone(), state.clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            use tauri::Manager;
            match event {
                // Close button hides to tray; the app lives on.
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    if window.label() == "mini" {
                        tray::hide_mini(window.app_handle());
                    } else {
                        tray::hide_popup(window.app_handle());
                    }
                }
                // A dragging mini summary settles into an anchor. Only moves
                // during a user drag count: anchoring and resizing move the
                // window too, and must never rewrite the user's choice.
                WindowEvent::Moved(_) => {
                    if window.label() != "mini" {
                        return;
                    }
                    let Some(state) = window.app_handle().try_state::<Arc<AppState>>() else {
                        return;
                    };
                    use std::sync::atomic::Ordering::Relaxed;
                    if !state.mini_dragging.load(Relaxed) {
                        return;
                    }
                    // A press that never moved leaves the flag armed — no Moved
                    // arrives to resolve it — so it also expires. Without this,
                    // the next anchoring move after such a press would be
                    // mistaken for a drag and rewrite the user's choice.
                    let since_press = chrono::Utc::now().timestamp_millis()
                        - state.last_mini_drag_ms.load(Relaxed);
                    if since_press > 30_000 {
                        state.mini_dragging.store(false, Relaxed);
                        return;
                    }
                    let gen = state.mini_drag_gen.fetch_add(1, Relaxed) + 1;
                    tray::settle_mini_drag(window.app_handle(), gen);
                }
                // Click-away dismiss (opt-in), suppressed right after a
                // title-bar press: starting a native drag on Windows drops
                // focus momentarily (tauri#10767) and must not hide the window.
                WindowEvent::Focused(false) => {
                    let Some(state) = window.app_handle().try_state::<Arc<AppState>>() else {
                        return;
                    };
                    use std::sync::atomic::Ordering::Relaxed;
                    // The mini summary is always click-away transient unless
                    // its own pin button is active. The full window keeps the
                    // separate user-configured click-away preference.
                    if window.label() == "mini" {
                        // Same drag grace period the full window needs: starting
                        // a native drag drops focus momentarily (tauri#10767),
                        // and dismissing the summary mid-drag would make it
                        // impossible to move to another screen.
                        let since_drag = chrono::Utc::now().timestamp_millis()
                            - state.last_mini_drag_ms.load(Relaxed);
                        if !state.mini_pinned.load(Relaxed) && since_drag > 2_000 {
                            tray::hide_mini(window.app_handle());
                        }
                        return;
                    }
                    if !state.hide_on_blur.load(Relaxed) {
                        return;
                    }
                    let since_drag =
                        chrono::Utc::now().timestamp_millis() - state.last_drag_ms.load(Relaxed);
                    if since_drag > 2_000 {
                        tray::hide_popup(window.app_handle());
                    }
                }
                _ => {}
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running quota-widget");
}

/// Re-export for poller/tray: emit fresh snapshots to the webview.
pub fn emit_snapshots(app: &tauri::AppHandle, snaps: &[UsageSnapshot]) {
    let _ = app.emit("snapshots", snaps);
}
