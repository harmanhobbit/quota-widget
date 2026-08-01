mod codex_oauth;
mod oauth;
mod poller;
mod secrets;
mod tray;
#[cfg(target_os = "linux")]
mod tray_linux;

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
    /// Millis timestamp of the last title-bar press — blur events within the
    /// grace window are drag-induced (tauri#10767), not click-away.
    pub last_drag_ms: std::sync::atomic::AtomicI64,
}

impl AppState {
    fn provider_ctx(&self, config: Config) -> ProviderCtx {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let secrets = secrets::load_all(&self.config_dir, &config);
        let mut ctx = ProviderCtx::new(home, secrets, config);
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
async fn get_snapshots(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Vec<UsageSnapshot>, String> {
    let map = state.snapshots.read().await;
    let cfg = state.config.read().await;
    // Stable registry order, enabled providers only.
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
    Ok(out)
}

#[tauri::command]
async fn get_config(state: tauri::State<'_, Arc<AppState>>) -> Result<Config, String> {
    Ok(state.config.read().await.clone())
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
    *state.config.write().await = config.clone();
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
    let _ = window.hide();
}

#[tauri::command]
fn set_pinned(state: tauri::State<'_, Arc<AppState>>, window: tauri::Window, pinned: bool) {
    state.hide_on_blur.store(
        if pinned {
            false
        } else {
            state.config.blocking_read().hide_on_blur
        },
        std::sync::atomic::Ordering::Relaxed,
    );
    let _ = window.set_always_on_top(pinned);
    if pinned {
        if let Some(main) = window.app_handle().get_webview_window("main") {
            tray::anchor_above_panel(&main);
        }
    }
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
        last_drag_ms: std::sync::atomic::AtomicI64::new(0),
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
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(state.clone())
        .invoke_handler(tauri::generate_handler![
            get_snapshots,
            get_config,
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
            set_pinned,
            note_drag,
            quit,
        ])
        .setup(move |app| {
            #[cfg(target_os = "linux")]
            tray_linux::create_tray(app.handle().clone(), state.clone());
            #[cfg(not(target_os = "linux"))]
            tray::create_tray(app.handle())?;
            poller::spawn(app.handle().clone(), state.clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            use tauri::Manager;
            match event {
                // Close button hides to tray; the app lives on.
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                // Click-away dismiss (opt-in), suppressed right after a
                // title-bar press: starting a native drag on Windows drops
                // focus momentarily (tauri#10767) and must not hide the window.
                WindowEvent::Focused(false) => {
                    let Some(state) = window.app_handle().try_state::<Arc<AppState>>() else {
                        return;
                    };
                    use std::sync::atomic::Ordering::Relaxed;
                    if !state.hide_on_blur.load(Relaxed) {
                        return;
                    }
                    let since_drag =
                        chrono::Utc::now().timestamp_millis() - state.last_drag_ms.load(Relaxed);
                    if since_drag > 2_000 {
                        let _ = window.hide();
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
