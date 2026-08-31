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
use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use tauri::{Emitter, Manager};
use tokio::sync::Mutex;

use crate::lan_pairing;

/// The foreground runtime's app handle, if this process hosts the app.
///
/// Background refresh work — the periodic ~15-minute job and the manual
/// one-time refresh (issue #111) — lands in `widget_jni.rs`'s headless refresh
/// whichever host enqueued it. When the app is alive in this same process, its
/// webview must hear about the new read model exactly as it does from
/// `refresh_once` (the `snapshots` event); when the process hosts only the
/// widget there is no webview, and persisting the read model is the whole
/// delivery. `widget_jni` checks this handle to tell the two apart.
static APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

/// The app handle if the Tauri runtime is alive in this process, else `None`.
pub fn app_handle() -> Option<&'static tauri::AppHandle> {
    APP_HANDLE.get()
}

/// The newest attempt generation whose results have been published to the
/// in-memory prior map and the open webview. The durable merge (under the
/// store lock) decides the order in which results reach disk; this gate makes
/// the *publication* order agree with it. Without it, a slow foreground pass
/// could store first and emit last — regressing cards a worker's newer
/// generation had already delivered — and a persist-failure fallback could do
/// the same against any writer that committed between its read and its emit.
static PUBLISHED_GENERATION: std::sync::Mutex<quota_core::snapshots::PublicationGate> =
    std::sync::Mutex::new(quota_core::snapshots::PublicationGate::new(0));

/// Publish the merged read model for `generation` to the in-memory prior map
/// and the open webview — unless an equal or newer generation was already
/// published, in which case the result is dropped and `false` is returned.
/// Shared by the foreground path and the WorkManager worker path (same
/// process, same gate); pass-specific delivery markers fire only on `true`.
///
/// The gate's verdict is held across BOTH the memory update and the emit: the
/// publication is one critical section. Admitting and releasing before the
/// delivery would let gen1 pause mid-publication while gen2 publishes fully
/// and then resume, regressing what gen2 delivered — the exact race the gate
/// exists to close.
pub fn publish_snapshots(
    app: &tauri::AppHandle,
    generation: u64,
    snapshots: Vec<UsageSnapshot>,
) -> bool {
    let mut gate = PUBLISHED_GENERATION
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    gate.publish(generation, || {
        if let Some(state) = app.try_state::<std::sync::Arc<MobileState>>() {
            let map = snapshots
                .iter()
                .map(|s| (s.provider_id.clone(), s.clone()))
                .collect();
            *state.snapshots.write().unwrap_or_else(|p| p.into_inner()) = map;
        }
        let _ = app.emit("snapshots", &snapshots);
    })
}

pub struct MobileState {
    pub config_dir: PathBuf,
    pub config: tokio::sync::RwLock<Config>,
    /// The in-memory prior map, seeded from the persisted read model at
    /// startup and republished by every refresh pass that wins the
    /// publication gate (see [`PUBLISHED_GENERATION`]). A std lock: it is
    /// only ever held for a clone/swap, never across an await, and the
    /// worker path publishes from a plain JNI thread.
    pub snapshots: std::sync::RwLock<HashMap<String, UsageSnapshot>>,
    pub alert_engine: Mutex<AlertEngine>,
    /// In-progress desktop→phone QR scan (issue #156): frames accumulate here
    /// across repeated `qr_scan_frame` calls, one per camera detection, until
    /// `qr_scan_finish` opens and applies the reassembled bundle.
    pub qr_collector: Mutex<quota_core::qr_transfer::FrameCollector>,
    /// The one armed LAN pairing session, whichever role it is (issues
    /// #154/#155): a receive wait or a send in flight. The same slot type,
    /// session rules and command set desktop uses — `lan_pairing.rs` owns
    /// them once for both hosts through `PairingHost`/`PairingState`.
    pub lan_pairing: std::sync::Mutex<crate::lan_pairing::SessionSlot>,
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
    // The std-lock guard must be dropped before the await below: a guard held
    // across an await makes the command's future non-Send.
    let persisted = {
        let map = state.snapshots.read().unwrap_or_else(|p| p.into_inner());
        map.clone()
    };
    let cfg = state.config.read().await;
    let mut out = Vec::new();
    for p in providers_for(&cfg) {
        if cfg
            .providers
            .get(p.id())
            .map(|c| c.enabled)
            .unwrap_or(false)
        {
            if let Some(s) = persisted.get(p.id()) {
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
    // Config::save takes the snapshot store lock itself: every configuration
    // persistence is coordinated with the merge's membership-authority
    // comparison — a save can never land between that comparison and the
    // merge's own save of the read model.
    config.save(&state.config_dir).map_err(|e| e.to_string())?;
    // Clear the alert memory of any account this config no longer enables, and
    // persist it immediately (issue #112): disabling or deleting an account
    // must start a fresh baseline when it returns, and a background worker could
    // fire before the next foreground refresh's own prune. `refresh` prunes too,
    // so this only brings that guarantee forward to the moment of the change.
    {
        let enabled: std::collections::HashSet<String> = config
            .providers
            .iter()
            .filter(|(_, p)| p.enabled)
            .map(|(id, _)| id.clone())
            .collect();
        let mut engine = state.alert_engine.lock().await;
        engine.retain_accounts(&enabled);
        if let Err(e) = engine.save(&state.config_dir) {
            eprintln!("[mobile] pruning alert memory failed: {e}");
        }
    }
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
/// with none of desktop's tray icon / notification presentation. This is the
/// foreground host's path — it runs only while the app is visible (entry and
/// the visibility-gated loop; the manual button goes through `refresh_manual`
/// so its work is durable). Background refresh opportunities belong to the
/// native host's WorkManager schedule (issue #111, ADR-0006).
#[tauri::command]
async fn refresh_now(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<MobileState>>,
) -> Result<(), String> {
    refresh_once(&app, &state).await;
    Ok(())
}

/// The app's manual refresh (issue #111): enqueue one-time durable WorkManager
/// work rather than fetch inline, so the refresh can finish independently of
/// the activity — the user is free to background or dismiss the app the moment
/// the tap lands and the fresh data still arrives. The worker persists the
/// read model and, when this process still hosts the app, announces it to the
/// webview with the same `snapshots` event `refresh_once` uses (see
/// `widget_jni::headless_refresh`).
///
/// Only the *foreground refresh loop* and entry refresh keep fetching
/// in-process via [`refresh_now`] — those run while the app is visible, where
/// an immediate in-process fetch is the point. If the durable enqueue itself
/// fails (a JNI/scheduler problem), fall back to the in-process refresh: a
/// failed tap must not simply do nothing.
#[tauri::command]
async fn refresh_manual(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<MobileState>>,
) -> Result<(), String> {
    #[cfg(target_os = "android")]
    match crate::android_schedule::enqueue_manual_refresh() {
        Ok(()) => {
            // The durable unit is now WorkManager's; surface the handoff, so a
            // device log distinguishes "enqueued and finished out-of-band"
            // from "fell back to the in-process fetch" below.
            eprintln!("[mobile] manual refresh enqueued as durable work");
            return Ok(());
        }
        Err(e) => eprintln!("[mobile] enqueueing manual refresh failed: {e}"),
    }
    refresh_once(&app, &state).await;
    Ok(())
}

async fn refresh_once(app: &tauri::AppHandle, state: &Arc<MobileState>) {
    // This pass's attempt generation — allocated from the persisted monotonic
    // counter under the store lock, never the wall clock (concurrent starts
    // can collide and clock adjustments go backwards; see `next_generation`).
    // Allocation failure fails the whole pass before anything is fetched,
    // merged, persisted, or published: an unorderable result must not be
    // written or shown, matching the headless worker's fail-closed behaviour
    // — silently proceeding with generation 0 (or a reused one) would make
    // this pass's results never apply to the model.
    let attempt = match quota_core::snapshots::next_generation(&state.config_dir) {
        Ok(generation) => generation,
        Err(e) => {
            eprintln!("[mobile] allocating refresh generation failed, skipping this refresh: {e}");
            return;
        }
    };
    let cfg = state.config.read().await.clone();
    let (ctx, failed_secrets) = state.provider_ctx_and_failed_secrets(cfg.clone());
    let prior = state
        .snapshots
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone();
    let mut engine = state.alert_engine.lock().await;
    let mut outcome = quota_core::refresh::refresh(&ctx, &prior, &mut engine).await;
    // Persist the alert memory this pass produced (edge-triggered levels,
    // baselines, and the prune to still-enabled accounts `refresh` applied) so
    // the next background worker or cold start measures crossings against it
    // rather than re-baselining and re-firing an unchanged state (issue #112).
    // The intact file is what survives process death, reboot and upgrade per
    // ADR-0006; a corrupt one is discarded and simply re-baselines.
    if let Err(e) = engine.save(&state.config_dir) {
        eprintln!("[mobile] persisting alert memory failed: {e}");
    }
    // Whether any account has ever read successfully — the engine's baseline
    // set surviving this pass is exactly the fact
    // `should_request_notification_permission` means by
    // `any_account_succeeded`. Read before the engine is dropped; the
    // contextual permission ask below keys off it.
    #[cfg(target_os = "android")]
    let any_account_succeeded = engine.has_baseline();
    drop(engine);

    // Hand this pass's planned notifications to the Kotlin host (issue #112):
    // quota-core has already filtered the events through the per-account toast
    // toggles and the baseline rule and produced the full/public content pair,
    // so all that crosses the boundary here is the finished JSON the host
    // posts. Best-effort like every other notification path — a failed
    // delivery is logged and never fails the refresh itself. The background
    // worker delivers its own plan the same way (see `widget_jni::
    // headless_refresh`); the two hosts never run the same pass, and the
    // engine's edge-triggering means a crossing notifies once, not twice.
    #[cfg(target_os = "android")]
    {
        let plan = quota_core::alerts::plan_notifications_json(&outcome, &cfg);
        if plan != "[]" {
            if let Err(e) = crate::android_notifications::deliver(&plan) {
                eprintln!("[mobile] delivering notification plan failed: {e}");
            }
        }
    }

    // The one contextual POST_NOTIFICATIONS ask (issue #112): only after some
    // account has read successfully (so the request has context), only when an
    // enabled account wants notifications, and only while the one-shot flag
    // (`notification_permission_requested`, durable in platform preferences)
    // says the request has never been issued — grant or denial alike ends the
    // asking forever, and Settings is the recovery path. The event asks the
    // open webview to invoke `request_notification_permission`, which fires
    // the actual system dialog from the foreground Activity; a background
    // worker never requests permission.
    #[cfg(target_os = "android")]
    if quota_core::alerts::should_request_notification_permission(
        &cfg,
        any_account_succeeded,
        cfg.notification_permission_requested,
    ) {
        let _ = app.emit("notification-permission-prompt", ());
    }

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
    // `ctx.secrets`), so replace it with the distinct `Unavailable` state so
    // the card says a storage failure happened, not that nothing was ever
    // configured. Ciphertext itself was never touched — only the read failed,
    // and the fix is to remove and re-paste (issue #133).
    if !failed_secrets.is_empty() {
        // A failed key is either the account's own key (a pasted-key provider)
        // or its `{id}_oauth` token entry (Claude/Codex sign-in). Match both, so
        // an undecryptable OAuth *token* is reported as unavailable rather than
        // slipping through as the generic "not configured" `refresh` produced.
        let affected = |pid: &str| {
            failed_secrets.iter().any(|k| {
                k == pid || crate::mobile_signin::provider_key_from_oauth_secret(k) == Some(pid)
            })
        };
        for provider in providers_for(&cfg) {
            if !affected(provider.id()) {
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
                quota_core::model::FetchError::Unavailable(
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

    // Re-establish display order after the post-refresh replacements above,
    // which `retain`+`push` any failed provider to the end of what `refresh`
    // had already sorted. The persisted read model and the webview event both
    // want the list render-ready.
    cfg.sort_snapshots(&mut outcome.snapshots);

    // Persist the read model so a cold process — the Activity relaunched, or
    // later a home-screen widget with no app running — renders this exact
    // last-known state without a live Tauri process. The store is written
    // through the generation-aware merge (`merge_and_store`): the foreground
    // composed this pass from its own possibly-stale view of `prior`, and a
    // concurrent writer — the WorkManager worker behind the manual or periodic
    // refresh, in this process or a widget-only one — may have persisted newer
    // figures while these fetches were in flight. The merge keeps the fresher
    // observation per provider, so a late partial failure here can add its
    // error but never erase a newer success's figures. The merged list — not
    // this pass's own outcome — is what the app keeps in memory and pushes to
    // the webview, so the cards reflect the store the next writer will build
    // on.
    // Publication is gated on the newest applied generation (see
    // PUBLISHED_GENERATION): whichever pass merged last under the store lock
    // is what memory and the webview must show, whatever the emit order.
    match quota_core::snapshots::SnapshotStore::merge_and_store(
        &state.config_dir,
        outcome.snapshots.clone(),
        attempt,
        &cfg,
    ) {
        Ok(store) => {
            publish_snapshots(&app, attempt, store.snapshots);
        }
        Err(e) => {
            eprintln!("[mobile] persisting snapshot read model failed: {e}");
            // The persist failed — but the raw outcome must never reach the
            // webview or memory: this pass may be causally older than what a
            // concurrent writer already stored, and publishing it unmerged
            // would visibly regress the open app. Derive the same causally
            // merged state the locked path would have written and publish
            // that through the same gate — with the store lock held across
            // BOTH the derivation and the publication, so no configuration
            // write can land between the membership check and what the
            // webview is given (the derivation and publication are one
            // atomic stretch against configuration writes, exactly like the
            // locked path). If the lock itself cannot be taken, publish
            // nothing: the cards keep their current state and the next pass
            // reconciles.
            match quota_core::snapshots::store_lock(&state.config_dir) {
                Ok(_lock) => {
                    let merged = quota_core::snapshots::SnapshotStore::derive_merged(
                        &state.config_dir,
                        outcome.snapshots.clone(),
                        attempt,
                        &cfg,
                    );
                    publish_snapshots(&app, attempt, merged.snapshots);
                }
                Err(lock_err) => {
                    eprintln!(
                        "[mobile] taking the store lock for the fallback derivation failed, \
                         skipping publication: {lock_err}"
                    );
                }
            }
        }
    }
}

// ---- Notification permission (issue #112) ----------------------------------

/// The platform's notification-permission state for the mobile Settings row:
/// `"granted"` or `"denied"` on Android 13+; `"granted"` below 13, where no
/// runtime permission exists. Off Android the bridge has no native half and
/// the error is surfaced to the UI as "unavailable".
#[tauri::command]
fn notification_permission_state() -> Result<String, String> {
    #[cfg(target_os = "android")]
    return crate::android_notifications::permission_state();
    #[cfg(not(target_os = "android"))]
    Err("notification permission state only exists on Android".into())
}

/// Fire the one-time POST_NOTIFICATIONS request (issue #112). The decision to
/// ask was made in `refresh_once` (first successful account + notifications
/// wanted) and surfaced to the open webview as `notification-permission-prompt`;
/// this command is its acknowledgement, invoked from the foreground app. It is
/// a durable one-shot: the request is issued to the system first, and only a
/// successful issuance flips `notification_permission_requested` in platform
/// preferences — a grant and a denial are recorded identically, so the user is
/// never asked twice, and the system settings link in Settings is the recovery
/// path either way.
#[tauri::command]
async fn request_notification_permission(
    state: tauri::State<'_, Arc<MobileState>>,
) -> Result<(), String> {
    #[cfg(not(target_os = "android"))]
    {
        let _ = &state;
        return Err("notification permission only exists on Android".into());
    }
    #[cfg(target_os = "android")]
    {
        // Checked under the config write lock and persisted in the same save
        // that records it, so two racing invocations cannot both issue the
        // system dialog on the strength of a stale flag.
        let mut cfg = state.config.write().await;
        if cfg.notification_permission_requested {
            return Ok(());
        }
        // Issue the request before persisting the flag: a bridge failure here
        // must not burn the one-shot. If the process dies between the dialog
        // and the save, the worst case is one extra ask after relaunch — the
        // opposite (a persisted flag with no dialog ever shown) would ask
        // silently never, which is the unrecoverable direction.
        crate::android_notifications::request_from_activity()?;
        let (shared, mut prefs) = cfg.split();
        prefs.notification_permission_requested = true;
        let updated = Config::from_parts(shared, prefs);
        updated.save(&state.config_dir).map_err(|e| e.to_string())?;
        *cfg = updated;
        Ok(())
    }
}

/// Open the system's notification settings for this app — the recovery path
/// once the one-time request is spent (denied, or later revoked): there is no
/// re-prompt by design, so the mobile Settings row links here instead.
#[tauri::command]
fn open_notification_settings() -> Result<(), String> {
    #[cfg(target_os = "android")]
    return crate::android_notifications::open_settings();
    #[cfg(not(target_os = "android"))]
    Err("notification settings only exist on Android".into())
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

// ---- Credential export & import (issue #152) -------------------------------
//
// Thin adapters over the `quota_core::transfer` / `quota_core::seal` seam —
// the same operations desktop's export (#151) uses, so a file written by one
// host opens on the other. Everything here is marshalling: the webview picks
// the file in a system dialog (dialog plugin), these commands move bytes
// between that file and the core, and no branching logic lives at this layer.

/// Reads a file the user picked in a system dialog. On Android that is a
/// `content://` URI, which `std::fs` cannot open — the fs plugin's `Fs::open`
/// resolves it through the ContentResolver into a real fd. A plain path still
/// works, which is what keeps this honest off Android.
fn read_user_file(app: &tauri::AppHandle, uri: &str) -> Result<Vec<u8>, String> {
    use tauri_plugin_fs::FsExt;
    // `FilePath::from_str`'s error type is `Infallible`: a string that does
    // not parse as a URI falls back to a plain path, never to an error.
    let path = tauri_plugin_fs::FilePath::from_str(uri).expect("FromStr is infallible");
    app.fs()
        .read(path)
        .map_err(|e| format!("could not read the chosen file: {e}"))
}

/// Writes bytes over a file the user created in the system save dialog.
/// Truncating mirrors the fs plugin's own `write_file`: the document SAF just
/// created is empty, but a rewrite must not leave a stale tail behind a
/// shorter export.
fn write_user_file(app: &tauri::AppHandle, uri: &str, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use tauri_plugin_fs::FsExt;
    let path = tauri_plugin_fs::FilePath::from_str(uri).expect("FromStr is infallible");
    let mut options = tauri_plugin_fs::OpenOptions::new();
    options.write(true).truncate(true);
    let mut file = app
        .fs()
        .open(path, options)
        .map_err(|e| format!("could not open the chosen file for writing: {e}"))?;
    file.write_all(bytes)
        .map_err(|e| format!("could not write the export: {e}"))
}

/// Export every account to the encrypted credential export the user just
/// chose in the save dialog (`destination` is its content URI on Android).
///
/// The bundle carries each account's entry plus, for pasted-key providers,
/// the API key read from the Keystore; OAuth/cookie accounts travel as shells
/// (ADR-0008) and sign in again on the importing device.
#[tauri::command]
async fn export_credentials(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<MobileState>>,
    destination: String,
    passphrase: String,
) -> Result<(), String> {
    let cfg = state.config.read().await.clone();
    let (shared, _prefs) = cfg.split();
    let dir = state.config_dir.clone();
    let bundle = quota_core::transfer::build_bundle(&shared, |key| crate::secrets::get(&dir, key));
    let bytes = quota_core::seal::seal(&bundle, &passphrase);
    write_user_file(&app, &destination, &bytes)
}

/// Import a credential export the user picked in the system file dialog,
/// merging its accounts into the current configuration and returning the
/// per-account report the UI summarises (added / updated / needs sign-in /
/// could-not-store).
///
/// The sealed bytes are opened *before* anything is touched, so a wrong
/// passphrase or a corrupt file is refused with existing accounts exactly as
/// they were. Pasted keys are written to the Android Keystore through the
/// same `secrets` backend Settings uses; `apply_bundle` leaves an account out
/// of the configuration when its key cannot be stored, so a Keystore failure
/// never leaves an account looking configured when it is not.
#[tauri::command]
async fn import_credentials(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<MobileState>>,
    source: String,
    passphrase: String,
) -> Result<quota_core::transfer::ApplyReport, String> {
    let bytes = read_user_file(&app, &source)?;
    apply_sealed_bytes(&app, &state, &bytes, &passphrase).await
}

/// Open sealed bytes under `passphrase` and merge their accounts into the
/// current configuration, returning the per-account report the UI summarises
/// (added / updated / needs sign-in / could-not-store). Shared by
/// `import_credentials` (bytes from a picked file, #152) and `qr_scan_finish`
/// (bytes reassembled from scanned QR frames, #156) — everything past "here
/// are the sealed bytes and a passphrase" is identical between the two
/// transports, and is now also exactly what the LAN pairing receiver does
/// with the bundle its PAKE channel already opened (`apply_opened_bundle`,
/// #155).
///
/// The sealed bytes are opened *before* anything is touched, so a wrong
/// passphrase or a corrupt/tampered payload is refused with existing
/// accounts exactly as they were.
async fn apply_sealed_bytes(
    app: &tauri::AppHandle,
    state: &Arc<MobileState>,
    bytes: &[u8],
    passphrase: &str,
) -> Result<quota_core::transfer::ApplyReport, String> {
    let bundle = quota_core::seal::open(bytes, passphrase).map_err(|e| e.to_string())?;
    apply_opened_bundle(app, state, &bundle).await
}

/// Merge an **already-opened** bundle into the current configuration — the
/// apply/commit seam every mobile transport shares (issue #155): the picked
/// file (#152) and the reassembled QR scan (#156) open their sealed bytes
/// with a passphrase and land here, and the LAN pairing receiver (#155)
/// arrives here directly, the PAKE channel key having already opened the
/// bundle inside `quota_core::pairing::receive_bundle`.
///
/// The recovery check is the first thing after the open, so in every case a
/// configuration the app cannot persist is refused before any secret is
/// written into a state the accounts themselves could not be saved into.
/// Pasted keys are written to the Android Keystore through the same
/// `secrets` backend Settings uses; `apply_bundle` leaves an account out of
/// the configuration when its key cannot be stored, so a Keystore failure
/// never leaves an account looking configured when it is not — the honest
/// could-not-store outcome the summary reports.
async fn apply_opened_bundle<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &Arc<MobileState>,
    bundle: &quota_core::transfer::CredentialBundle,
) -> Result<quota_core::transfer::ApplyReport, String> {
    // A configuration in recovery (an unreadable file the app is carefully
    // not overwriting) must fail here, before any secret is written into a
    // state the accounts themselves could not be persisted into — `save`
    // would refuse below either way, but by then the Keystore writes would
    // already have happened.
    if let Some(recovery) = Config::recovery_state(&state.config_dir) {
        return Err(format!(
            "the existing configuration could not be read ({}), so the import was refused \
             rather than risk overwriting it",
            recovery.detail
        ));
    }
    let cfg = state.config.read().await.clone();
    let (mut shared, prefs) = cfg.split();
    let dir = state.config_dir.clone();
    let report = quota_core::transfer::apply_bundle(bundle, &mut shared, |key, value| {
        crate::secrets::set(&dir, key, value)
    });
    // A transferred account keeps its source's settings verbatim, including a
    // desktop `auth_mode: "cli"` that cannot work on Android. Rewrite it to the
    // built-in sign-in before this config is persisted, so a phone that just
    // received a desktop bundle doesn't land a permanently-stuck account.
    quota_core::config::coerce_cli_auth_mode_to_oauth(&mut shared.providers);
    // Persist only when at least one account actually landed: an import whose
    // every account failed to store leaves the configuration file untouched.
    let landed = report.accounts.values().any(|outcome| {
        !matches!(
            outcome,
            quota_core::transfer::ApplyOutcome::CouldNotStore { .. }
        )
    });
    if landed {
        let updated = Config::from_parts(shared, prefs);
        updated.save(&state.config_dir).map_err(|e| e.to_string())?;
        *state.config.write().await = updated.clone();
        let _ = app.emit("config", &updated);
    }
    Ok(report)
}

// ---- LAN pairing (issue #155) ----------------------------------------------
//
// The same live transport desktop pairing uses, one desktop↔phone or
// desktop↔desktop exchange at a time. The session rules — one armed slot per
// code, bounded and cancelable either role, disarm-before-apply — live in
// `lan_pairing.rs` written once for both hosts; this host only supplies the
// two things it alone knows: how to gather its accounts into a bundle, and
// `apply_opened_bundle` as the receiver's landing place.

impl crate::lan_pairing::PairingHost for Arc<MobileState> {
    fn pairing_slot(&self) -> &std::sync::Mutex<crate::lan_pairing::SessionSlot> {
        &self.lan_pairing
    }

    async fn build_bundle(&self) -> quota_core::transfer::CredentialBundle {
        let cfg = self.config.read().await.clone();
        let (shared, _prefs) = cfg.split();
        let dir = self.config_dir.clone();
        quota_core::transfer::build_bundle(&shared, |key| crate::secrets::get(&dir, key))
    }

    async fn apply_received<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
        bundle: &quota_core::transfer::CredentialBundle,
    ) -> Result<quota_core::transfer::ApplyReport, String> {
        apply_opened_bundle(app, self, bundle).await
    }
}

// ---- Desktop→phone QR transfer (issue #156) --------------------------------
//
// The webview scans QR frames one at a time via the barcode-scanner plugin's
// own JS API and hands each decoded string to `qr_scan_frame`, which folds it
// into the in-progress `FrameCollector` — the same pure reassembly logic
// `quota_core::qr_transfer` is unit-tested against. Once collection reports
// complete, `qr_scan_finish` opens and applies the reassembled bytes through
// the exact path `import_credentials` uses for a picked file.

/// Start a fresh scan: discards any frames collected so far. Call when the
/// scan screen opens, and again on retry — a scan abandoned mid-way must not
/// leave stale frames around to confuse the next attempt.
#[tauri::command]
async fn qr_scan_reset(state: tauri::State<'_, Arc<MobileState>>) -> Result<(), String> {
    *state.qr_collector.lock().await = quota_core::qr_transfer::FrameCollector::new();
    Ok(())
}

/// Feed one scanned QR frame's decoded text into the in-progress collection.
/// A frame that doesn't parse as ours (a stray unrelated QR code the camera
/// picked up) leaves progress unchanged rather than erroring, so the scan
/// loop never has to special-case it — it just keeps scanning.
#[tauri::command]
async fn qr_scan_frame(
    state: tauri::State<'_, Arc<MobileState>>,
    text: String,
) -> Result<quota_core::qr_transfer::FrameStatus, String> {
    let mut collector = state.qr_collector.lock().await;
    let status = collector.accept(&text).or_else(|_| {
        collector
            .status()
            .ok_or_else(|| "no frame scanned yet".to_string())
    });
    // Nothing scanned yet and this one didn't parse either: still "keep
    // scanning", not a fatal error — report zero progress rather than fail.
    Ok(status.unwrap_or(quota_core::qr_transfer::FrameStatus {
        have: 0,
        total: 0,
        complete: false,
    }))
}

/// Once scanning is complete, open the reassembled sealed bytes under
/// `passphrase` and apply them — identical outcome shape to
/// `import_credentials`. Resets the collector afterwards either way, so a
/// wrong passphrase can be retried by re-entering it without rescanning.
#[tauri::command]
async fn qr_scan_finish(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<MobileState>>,
    passphrase: String,
) -> Result<quota_core::transfer::ApplyReport, String> {
    let bytes = {
        let collector = state.qr_collector.lock().await;
        collector
            .assemble()
            .ok_or_else(|| "the scan is not complete yet".to_string())?
    };
    let result = apply_sealed_bytes(&app, &state, &bytes, &passphrase).await;
    *state.qr_collector.lock().await = quota_core::qr_transfer::FrameCollector::new();
    result
}

#[tauri::mobile_entry_point]
pub fn run() {
    tauri::Builder::default()
        // First registered plugin on the mobile host (issue #159): built-in
        // sign-in hands the authorize/verification URL to `opener::open_url`
        // via the injected JS API, which on Android resolves to a new-task
        // `ACTION_VIEW` intent (the external default browser) rather than the
        // in-app WebView — see the mobile capability's `opener:allow-open-url`
        // permission grant below.
        .plugin(tauri_plugin_opener::init())
        // Credential export/import (issue #152): the webview picks the export
        // file in the system dialog (this is what SAF's ACTION_GET_CONTENT /
        // ACTION_CREATE_DOCUMENT come through), and the export/import
        // commands below read and write the picked content URI through the fs
        // plugin's Rust API — its Kotlin `getFileDescriptor` resolves a
        // content:// URI via the ContentResolver, which std::fs cannot do.
        // Neither plugin is called over IPC by the webview beyond the two
        // dialog entry points granted in the mobile capability.
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        // Camera-based QR scanning for the desktop→phone transfer (issue
        // #156): the webview drives `scan()`/`cancel()` from the plugin's own
        // JS API directly (not an app-defined command), which is what the
        // `barcode-scanner:*` grants in the mobile capability gate.
        .plugin(tauri_plugin_barcode_scanner::init())
        .setup(move |app| {
            // Register the Android Keystore-backed credential store as
            // keyring-core's default before any secret access. The `keyring`
            // crate's `v1` facade has no `target_os = "android"` arm in its
            // one-time store initializer, so without this every `secrets::set`
            // / `get` / `clear` short-circuits to `NoDefaultStore` before
            // touching the Keystore (which is why pasting an OpenRouter key
            // on Android failed with "No default storage has been set"). See
            // `secrets::init_store`. A no-op on every non-Android target
            // `mobile.rs` is compiled for, so the iOS build calls it too
            // without effect.
            if let Err(e) = crate::secrets::init_store() {
                eprintln!("[mobile] keystore init failed: {e}");
            }
            // Remember the foreground runtime so the durable refresh work (the
            // periodic job and the manual one-time refresh, issue #111) can
            // reach this process's webview with its `snapshots` event — see
            // `widget_jni::headless_refresh`. Set before anything schedules
            // work; the widget-only process never runs this, so its handle
            // stays absent there.
            let _ = APP_HANDLE.set(app.handle().clone());
            // Issue #111: the best-effort periodic refresh is native host work
            // (ADR-0006) — make sure the ~15-minute WorkManager schedule exists
            // at every app start. Idempotent on the Kotlin side (unique work,
            // KEEP), and a failure here degrades to foreground-only refresh
            // rather than aborting startup: the same guarantee the widget
            // receiver path gives by calling `ensurePeriodic` itself.
            #[cfg(target_os = "android")]
            if let Err(e) = crate::android_schedule::ensure_periodic() {
                eprintln!("[mobile] scheduling periodic background refresh failed: {e}");
            }
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
            // Android has no CLI for any provider, so an `auth_mode: "cli"`
            // account — carried in verbatim from a desktop config by a
            // credential transfer, or left by an older build — can never fetch
            // and, with the sign-in-method selector removed from the mobile UI,
            // can never be corrected by the user. Coerce it to the built-in
            // sign-in the phone actually uses and persist, so both this
            // launch's poller and the next launch see the corrected mode.
            if quota_core::config::coerce_cli_auth_mode_to_oauth(&mut loaded.config.providers) {
                if let Err(e) = loaded.config.save(&config_dir) {
                    eprintln!("[mobile] persisting auth-mode normalization failed: {e}");
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
                    // Silences the foreground visibility loop for the emulator
                    // check (scripts/android-emulator-check.sh): with an hourly
                    // interval it cannot fire mid-check, so the card's data age
                    // only advances, and the age reset after the check's tap is
                    // attributable to exactly one thing — the manual refresh's
                    // durable worker pushing snapshots to the open webview.
                    loaded.config.poll_interval_secs = 3600;
                    if let Err(e) = loaded.config.save(&config_dir) {
                        eprintln!("[ci-seed] saving seeded config failed: {e}");
                    }
                }
                None => {
                    eprintln!("[ci-seed] no OPENROUTER_CI_KEY baked in (ci_test_key() == None)")
                }
            }
            // Seed the in-memory snapshots from the persisted read model so the
            // very first `get_snapshots` (the webview mounting) renders the
            // last-known state a previous process left behind — before this
            // launch's own refresh has completed — and so that refresh receives
            // those figures as `prior` and can keep them visibly stale if its
            // first fetch fails, rather than blanking a card on every cold
            // start. A missing or corrupt file loads as an empty read model
            // (derived data is discarded, not recovered — see
            // `quota_core::snapshots`).
            let persisted = quota_core::snapshots::SnapshotStore::load(&config_dir);
            // Seed the alert memory from disk too, so this process measures
            // crossings against what earlier processes (a background worker, the
            // previous app launch) already saw rather than re-baselining and
            // re-firing an unchanged warning/critical (issue #112). A missing or
            // corrupt file loads as the empty memory — derived data, re-baselined
            // safely (see `quota_core::alerts::AlertEngine::load`).
            let alert_engine = AlertEngine::load(&config_dir);
            let state = Arc::new(MobileState {
                config_dir,
                config: tokio::sync::RwLock::new(loaded.config),
                snapshots: std::sync::RwLock::new(persisted.prior_map()),
                alert_engine: Mutex::new(alert_engine),
                qr_collector: Mutex::new(quota_core::qr_transfer::FrameCollector::new()),
                lan_pairing: std::sync::Mutex::new(crate::lan_pairing::SessionSlot::default()),
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
            refresh_manual,
            test_provider,
            notification_permission_state,
            request_notification_permission,
            open_notification_settings,
            start_claude_signin,
            finish_claude_signin,
            start_codex_signin,
            poll_codex_signin,
            cancel_signin,
            get_pending_signins,
            ci_test_key,
            export_credentials,
            import_credentials,
            qr_scan_reset,
            qr_scan_frame,
            qr_scan_finish,
            // LAN pairing (issues #154/#155): the same commands desktop
            // registers, written once in `lan_pairing.rs` against this
            // host's state. The receiver reports through the `lan-pairing`
            // event, exactly as on desktop.
            lan_pairing::lan_pairing_address,
            lan_pairing::lan_pairing_generate_code,
            lan_pairing::lan_pairing_send,
            lan_pairing::lan_pairing_receive_start,
            lan_pairing::lan_pairing_cancel,
        ])
        .run(tauri::generate_context!())
        .expect("error while running quota-widget (mobile)");
}
