//! Android's narrow JNI façade.
//!
//! Tauri's `mobile_entry_point` plus its generated `MainActivity` *is* the
//! JNI boundary here — this module is not hand-rolled JNI, it is the same
//! `invoke` → `#[tauri::command]` plumbing desktop uses, pointed at a
//! deliberately small command set with none of desktop's tray/window/
//! autostart concerns. Every command below calls straight into `quota_core`
//! (`refresh::refresh`, `Config`, `providers::providers_for`) — no provider
//! or quota logic is reimplemented here, satisfying issue #108's acceptance
//! criterion that the shared refresh operation is what produces quota, not a
//! Kotlin/JS reimplementation.
//!
//! Issue #109 added Keystore-backed secret encryption (`secrets.rs`'s
//! `target_os = "android"` backend), empty-first-run provider onboarding, and
//! multi-account CRUD for every direct-HTTPS pasted-key provider — desktop's
//! CLI/OAuth/SSH-only providers (Claude, Codex, Grok, Hermes) stayed excluded.
//! Issue #110 adds built-in Claude PKCE sign-in, Codex device-flow sign-in,
//! and Hermes cookie mode, plus encrypted pending-sign-in persistence and
//! honest handling when a rotated credential cannot be stored.

use quota_core::alerts::AlertEngine;
use quota_core::config::Config;
use quota_core::model::UsageSnapshot;
use quota_core::providers::{providers_for, ProviderCtx};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tokio::sync::{Mutex, RwLock};

pub struct MobileState {
    pub config_dir: PathBuf,
    pub config: RwLock<Config>,
    pub snapshots: RwLock<HashMap<String, UsageSnapshot>>,
    pub alert_engine: Mutex<AlertEngine>,
}

impl MobileState {
    fn provider_ctx(&self, config: Config) -> ProviderCtx {
        let (ctx, _failed) = self.provider_ctx_and_failed_secrets(config);
        ctx
    }

    /// Builds the refresh context plus the config keys whose stored
    /// credential exists but could not be decrypted (Android Keystore only —
    /// see `secrets.rs`). Those keys are deliberately left out of `ctx.secrets`
    /// (an adapter must not be handed a truncated/garbage credential), and
    /// `refresh_once` turns them into an explicit failed snapshot instead of
    /// the generic "not configured" a merely-absent secret would produce.
    fn provider_ctx_and_failed_secrets(&self, config: Config) -> (ProviderCtx, Vec<String>) {
        // No `dirs::home_dir()` on Android — nothing here reads it, since
        // Android's provider set is direct-HTTPS pasted-key only (no CLI
        // file, no SSH, no Tailscale), but ProviderCtx still needs a `home`
        // path; the config dir is as good a stand-in as any and is never
        // dereferenced by any provider Android exposes.
        let (secrets, failed) =
            crate::secrets::load_all_reporting_errors(&self.config_dir, &config);
        let mut ctx = ProviderCtx::new(
            self.config_dir.clone(),
            self.config_dir.clone(),
            secrets,
            config,
        );
        let dir = self.config_dir.clone();
        ctx.on_secret_update = Some(std::sync::Arc::new(move |key: &str, value: &str| {
            crate::secrets::set(&dir, key, value)
        }));
        (ctx, failed)
    }
}

#[derive(serde::Serialize)]
struct InitialState {
    snapshots: Vec<UsageSnapshot>,
    config: Config,
}

#[tauri::command]
async fn get_snapshots(state: tauri::State<'_, Arc<MobileState>>) -> Result<InitialState, String> {
    let map = state.snapshots.read().await;
    let cfg = state.config.read().await;
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
    cfg.sort_snapshots(&mut out);
    Ok(InitialState {
        snapshots: out,
        config: cfg.clone(),
    })
}

/// Save and broadcast only — no tray/mini-anchor/autostart calls, none of
/// which exist on mobile.
#[tauri::command]
async fn set_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<MobileState>>,
    config: Config,
) -> Result<(), String> {
    config.save(&state.config_dir).map_err(|e| e.to_string())?;
    *state.config.write().await = config.clone();
    let _ = app.emit("config", &config);
    Ok(())
}

#[tauri::command]
fn set_secret(
    state: tauri::State<'_, Arc<MobileState>>,
    provider: String,
    value: String,
) -> Result<(), String> {
    crate::secrets::set(&state.config_dir, &provider, &value)
}

#[tauri::command]
fn has_secret(state: tauri::State<'_, Arc<MobileState>>, provider: String) -> bool {
    crate::secrets::get(&state.config_dir, &provider)
        .map(|s| !s.is_empty())
        .unwrap_or(false)
}

#[tauri::command]
fn clear_secret(state: tauri::State<'_, Arc<MobileState>>, provider: String) -> Result<(), String> {
    crate::secrets::clear(&state.config_dir, &provider)
}

/// One pass of the shared refresh operation, presented the same way
/// `poller.rs` does for desktop (update snapshots, emit to the webview) but
/// with none of desktop's tray icon / notification presentation — Android's
/// alerts and background scheduling are later tickets.
#[tauri::command]
async fn refresh_now(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<MobileState>>,
) -> Result<(), String> {
    refresh_once(&app, &state).await;
    Ok(())
}

async fn refresh_once(app: &tauri::AppHandle, state: &Arc<MobileState>) {
    let cfg = state.config.read().await.clone();
    let (ctx, failed_secrets) = state.provider_ctx_and_failed_secrets(cfg.clone());
    let prior = state.snapshots.read().await.clone();
    let mut engine = state.alert_engine.lock().await;
    let mut outcome = quota_core::refresh::refresh(&ctx, &prior, &mut engine).await;
    drop(engine);

    // A rotated credential that could not be persisted is an authentication/
    // storage failure, not a healthy refresh. Keep any prior successful reading
    // visible as stale and mark the affected provider so the user knows to
    // reauthenticate rather than silently losing the rotation on process exit.
    for write in &outcome.credential_writes {
        if let Err(e) = &write.result {
            if let Some(provider_id) =
                crate::mobile_signin::provider_key_from_oauth_secret(&write.key)
            {
                let err = quota_core::model::FetchError::AuthExpired(format!(
                    "rotated credential could not be stored: {e}. Sign in again."
                ));
                let replacement = match prior.get(provider_id) {
                    Some(prev)
                        if prev.error.is_none()
                            && (!prev.windows.is_empty() || prev.credits.is_some()) =>
                    {
                        let mut merged = prev.clone();
                        merged.error = Some(err);
                        merged.fetched_at = chrono::Utc::now();
                        merged
                    }
                    _ => {
                        let name = providers_for(&cfg)
                            .iter()
                            .find(|p| p.id() == provider_id)
                            .map(|p| p.name().to_string())
                            .unwrap_or_else(|| provider_id.to_string());
                        UsageSnapshot::failed(provider_id, &name, err)
                    }
                };
                outcome.snapshots.retain(|s| s.provider_id != provider_id);
                outcome.snapshots.push(replacement);
            } else {
                eprintln!("failed to persist rotated secret {}: {e}", write.key);
            }
        }
    }

    // A stored credential that exists but could not be decrypted must not
    // read as "no key was ever pasted" — `refresh` above already produced a
    // generic `NotConfigured` for these (their secret was simply absent from
    // `ctx.secrets`), so replace it with a snapshot that says what actually
    // happened. Ciphertext itself was never touched — only the read failed.
    if !failed_secrets.is_empty() {
        for provider in providers_for(&cfg) {
            if !failed_secrets.contains(&provider.id().to_string()) {
                continue;
            }
            if !cfg
                .providers
                .get(provider.id())
                .map(|p| p.enabled)
                .unwrap_or(false)
            {
                continue;
            }
            let failed = UsageSnapshot::failed(
                provider.id(),
                provider.name(),
                quota_core::model::FetchError::NotConfigured(
                    "stored credential could not be decrypted — remove and re-paste it in \
                     Settings"
                        .into(),
                ),
            );
            outcome
                .snapshots
                .retain(|s| s.provider_id != failed.provider_id);
            outcome.snapshots.push(failed);
        }
    }

    {
        let mut map = state.snapshots.write().await;
        for s in &outcome.snapshots {
            map.insert(s.provider_id.clone(), s.clone());
        }
    }

    let _ = app.emit("snapshots", &outcome.snapshots);
}

/// One-off fetch for the mobile Settings "Test" button, mirroring desktop's
/// `test_provider`: ignores the enabled flag so an account can be verified
/// before switching it on.
#[tauri::command]
async fn test_provider(
    state: tauri::State<'_, Arc<MobileState>>,
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

// ---- Built-in sign-in commands (issue #110) --------------------------------

#[tauri::command]
async fn start_claude_signin(
    state: tauri::State<'_, Arc<MobileState>>,
    provider: String,
) -> Result<String, String> {
    crate::mobile_signin::start_claude(&state.config_dir, &provider).await
}

#[tauri::command]
async fn finish_claude_signin(
    state: tauri::State<'_, Arc<MobileState>>,
    provider: String,
    code: String,
) -> Result<(), String> {
    crate::mobile_signin::finish_claude(&state.config_dir, &provider, &code).await
}

#[tauri::command]
async fn start_codex_signin(
    state: tauri::State<'_, Arc<MobileState>>,
    provider: String,
) -> Result<crate::mobile_signin::CodexLoginInfo, String> {
    crate::mobile_signin::start_codex(&state.config_dir, &provider).await
}

#[tauri::command]
async fn poll_codex_signin(
    state: tauri::State<'_, Arc<MobileState>>,
    provider: String,
) -> Result<crate::mobile_signin::CodexPollResult, String> {
    crate::mobile_signin::poll_codex(&state.config_dir, &provider).await
}

#[tauri::command]
fn cancel_signin(
    state: tauri::State<'_, Arc<MobileState>>,
    provider: String,
) -> Result<(), String> {
    crate::mobile_signin::cancel(&state.config_dir, &provider)
}

#[tauri::command]
fn get_pending_signins(
    state: tauri::State<'_, Arc<MobileState>>,
) -> Result<Vec<crate::mobile_signin::PendingSignInView>, String> {
    crate::mobile_signin::list(&state.config_dir)
}

/// CI-only seed path — see `.github/workflows/build.yml`'s `android` job.
/// In a debug build this reads a value baked in at compile time via
/// `option_env!` from an environment variable the CI job sets; in a release
/// build the function is compiled to an unconditional `None` and never reads
/// the environment at all — registered unconditionally (rather than
/// cfg-gated out of the handler list) so both builds expose the same command
/// surface and only the debug build can ever answer with a real key.
#[tauri::command]
fn ci_test_key() -> Option<&'static str> {
    #[cfg(debug_assertions)]
    {
        option_env!("OPENROUTER_CI_KEY")
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

#[tauri::mobile_entry_point]
pub fn run() {
    tauri::Builder::default()
        .setup(move |app| {
            let config_dir = app
                .path()
                .app_config_dir()
                .unwrap_or_else(|_| std::env::temp_dir());
            // A true first run — neither storage file exists yet — must start
            // with an empty account list and provider onboarding (issue #109,
            // docs/adr/0006-…), not desktop's `Config::default()`, which
            // pre-enables Claude and Codex: Android has no CLI to discover
            // credentials from, so those would only ever render as failed
            // accounts before the user has even opened Settings. Checked
            // before `Config::load` runs any one-time file migration, so a
            // config directory carried over from an older build (which *did*
            // once write a legacy `config.json`) is never mistaken for a
            // first run.
            // "shared-config.json" mirrors quota_core::shared_config's
            // internal (crate-private) file name — see `Config::load`'s
            // `migrate_if_needed`, which is the authoritative check this one
            // stands in for.
            let is_first_run = !config_dir.join("shared-config.json").exists()
                && !config_dir.join("config.json").exists();
            let mut loaded = Config::load(&config_dir);
            if is_first_run {
                loaded.config = Config::mobile_first_run_default();
                if let Err(e) = loaded.config.save(&config_dir) {
                    eprintln!("[mobile] saving first-run default config failed: {e}");
                }
            }
            // CI-only OpenRouter seed for issue #108's emulator proof. Done
            // here in Rust — before the webview loads — rather than in the JS
            // onMount, because Tauri's Android webview reloads once during
            // startup and drops any in-flight `invoke` callbacks (logcat:
            // "[TAURI] Couldn't find callback id … app is reloaded while Rust
            // is running an asynchronous operation"), which kept cutting the JS
            // seed's persist→refresh chain short so no account was ever stored.
            // Seeding synchronously at setup means the account exists before
            // the first `refresh_once` below, and the eprintln makes it
            // unambiguous in logcat whether the key was actually baked in.
            // `ci_test_key()` is compiled to `None` in release builds, so this
            // whole block vanishes there.
            match ci_test_key() {
                Some(key) => {
                    eprintln!("[ci-seed] OPENROUTER_CI_KEY present; seeding openrouter account");
                    if let Err(e) = crate::secrets::set(&config_dir, "openrouter", key) {
                        eprintln!("[ci-seed] storing openrouter secret failed: {e}");
                    }
                    let account = quota_core::config::ProviderConfig {
                        kind: Some("openrouter".to_string()),
                        enabled: true,
                        ..Default::default()
                    };
                    loaded
                        .config
                        .providers
                        .insert("openrouter".to_string(), account);
                    if let Err(e) = loaded.config.save(&config_dir) {
                        eprintln!("[ci-seed] saving seeded config failed: {e}");
                    }
                }
                None => {
                    eprintln!("[ci-seed] no OPENROUTER_CI_KEY baked in (ci_test_key() == None)")
                }
            }
            let state = Arc::new(MobileState {
                config_dir,
                config: RwLock::new(loaded.config),
                snapshots: RwLock::new(HashMap::new()),
                alert_engine: Mutex::new(AlertEngine::default()),
            });
            app.manage(state.clone());
            // Opens directly to the usage list, so the first thing on screen
            // needs live data rather than an empty card list.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                refresh_once(&handle, &state).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshots,
            set_config,
            set_secret,
            has_secret,
            clear_secret,
            refresh_now,
            test_provider,
            start_claude_signin,
            finish_claude_signin,
            start_codex_signin,
            poll_codex_signin,
            cancel_signin,
            get_pending_signins,
            ci_test_key,
        ])
        .run(tauri::generate_context!())
        .expect("error while running quota-widget (mobile)");
}
