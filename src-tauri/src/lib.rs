mod poller;
mod secrets;
mod tray;

use quota_core::alerts::AlertEngine;
use quota_core::config::Config;
use quota_core::model::UsageSnapshot;
use quota_core::providers::{all_providers, ProviderCtx};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, WindowEvent};
use tauri_plugin_autostart::ManagerExt;
use tokio::sync::{Mutex, Notify, RwLock};

pub struct AppState {
    pub config_dir: PathBuf,
    pub config: RwLock<Config>,
    pub snapshots: RwLock<HashMap<String, UsageSnapshot>>,
    pub alert_engine: Mutex<AlertEngine>,
    /// Poked to trigger an immediate poll cycle.
    pub refresh: Notify,
}

impl AppState {
    fn provider_ctx(&self, config: Config) -> ProviderCtx {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let secrets = secrets::load_all(&self.config_dir);
        ProviderCtx::new(home, secrets, config)
    }
}

fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(std::env::temp_dir).join("quota-widget")
}

// ---- IPC commands -----------------------------------------------------------

#[tauri::command]
async fn get_snapshots(state: tauri::State<'_, Arc<AppState>>) -> Result<Vec<UsageSnapshot>, String> {
    let map = state.snapshots.read().await;
    let cfg = state.config.read().await;
    // Stable registry order, enabled providers only.
    let mut out = Vec::new();
    for p in all_providers() {
        if cfg.providers.get(p.id()).map(|c| c.enabled).unwrap_or(false) {
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
    *state.config.write().await = config.clone();
    // Apply autostart immediately.
    let autolaunch = app.autolaunch();
    let result = if config.autostart { autolaunch.enable() } else { autolaunch.disable() };
    if let Err(e) = result {
        eprintln!("autostart: {e}"); // non-fatal (e.g. unsupported desktop)
    }
    state.refresh.notify_one();
    Ok(())
}

#[tauri::command]
fn set_secret(state: tauri::State<'_, Arc<AppState>>, provider: String, value: String) -> Result<(), String> {
    secrets::set(&state.config_dir, &provider, &value)
}

#[tauri::command]
fn has_secret(state: tauri::State<'_, Arc<AppState>>, provider: String) -> bool {
    secrets::get(&state.config_dir, &provider).map(|s| !s.is_empty()).unwrap_or(false)
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
    let ctx = state.provider_ctx(cfg);
    for p in all_providers() {
        if p.id() == provider {
            return Ok(match p.fetch(&ctx).await {
                Ok(s) => s,
                Err(e) => UsageSnapshot::failed(p.id(), p.name(), e),
            });
        }
    }
    Err(format!("unknown provider: {provider}"))
}

#[tauri::command]
fn hide_window(window: tauri::Window) {
    let _ = window.hide();
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
        config: RwLock::new(config),
        snapshots: RwLock::new(HashMap::new()),
        alert_engine: Mutex::new(AlertEngine::default()),
        refresh: Notify::new(),
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
            hide_window,
            quit,
        ])
        .setup(move |app| {
            tray::create_tray(app.handle())?;
            poller::spawn(app.handle().clone(), state.clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                // Close button hides to tray; the app lives on.
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                // Clicking elsewhere dismisses the popup.
                WindowEvent::Focused(false) => {
                    let _ = window.hide();
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
