//! JNI entry points for the native Glance home-screen widget host (issue #113).
//!
//! The launcher hosts the widget in this app's process but *without* the Tauri
//! runtime necessarily alive — a cold widget must render with no activity open.
//! Rather than re-implement any quota decision in Kotlin (ADR-0006: the host is
//! thin, `quota-core` owns behaviour), the Kotlin host calls straight into this
//! shared library, which is already loaded as `libquota_widget_lib.so`:
//!
//! - [`Java_tech_allaway_quotawidget_widget_WidgetBridge_nativeRender`] projects
//!   one instance into the [`WidgetView`](quota_core::widget_view::WidgetView)
//!   JSON the host draws — the cold, credential-free read.
//! - `nativeConfigOptions` / `nativeSaveInstance` / `nativeRemoveInstance` drive
//!   the placement configuration activity.
//! - `nativeRefresh` is the body of every durable refresh work item — the
//!   one-time job the widget's refresh action and the app's manual refresh
//!   enqueue, and the periodic ~15-minute schedule (issue #111): a real
//!   headless refresh (load config, decrypt Keystore secrets, fetch, persist
//!   the read model) that runs to completion with no activity open.
//!
//! All the projection, flattening, redaction and persistence logic lives in
//! `quota-core` and is exercised by that crate's Linux unit tests; this file is
//! only the marshalling of strings and numbers across the JNI boundary, kept
//! deliberately mechanical so nothing testable hides on the Android-only side.
//!
//! Every entry point catches its errors and returns them as a string (an empty
//! string means success for the write/refresh calls) so a Rust panic never
//! unwinds across the JNI boundary into the JVM.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use jni::objects::{JClass, JObject, JString};
use jni::sys::{jdouble, jlong, jstring};
use jni::JNIEnv;
use tauri::Emitter;

use quota_core::alerts::AlertEngine;
use quota_core::config::{Config, ConfigPresence};
use quota_core::providers::ProviderCtx;
use quota_core::snapshots::SnapshotStore;
use quota_core::widget_view;

/// Pull a `JString` argument into an owned Rust `String`, or an empty string if
/// the JVM handed us a null/garbage reference (which the callers treat as a
/// missing directory/instance and degrade from, never crash on).
fn jstring_arg(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s)
        .map(|js| js.into())
        .unwrap_or_else(|_| String::new())
}

/// Build a Java string to hand back, falling back to an empty JVM string if the
/// allocation fails — never an unwrap that could panic across the boundary.
fn to_jstring(env: &mut JNIEnv, value: &str) -> jstring {
    env.new_string(value)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// The `chrono` instant from a Java epoch-milliseconds `long`, clamping a
/// nonsensical value to "now" so a bad clock reading never skews the caption.
fn now_from_millis(now_millis: jlong) -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(now_millis)
        .single()
        .unwrap_or_else(Utc::now)
}

/// `WidgetBridge.nativeRender(dir, instanceId, widthDp, heightDp, nowMillis)`:
/// the cold read. Returns the [`WidgetView`](quota_core::widget_view::WidgetView)
/// JSON for the launcher to draw. Never fetches and never reads a credential.
///
/// # Safety
/// A JNI entry point: the JVM guarantees the argument references are valid for
/// the call. All Rust errors are caught and marshalled, so nothing unwinds.
#[no_mangle]
pub extern "system" fn Java_tech_allaway_quotawidget_widget_WidgetBridge_nativeRender(
    mut env: JNIEnv,
    _class: JClass,
    dir: JString,
    instance_id: JString,
    width_dp: jdouble,
    height_dp: jdouble,
    now_millis: jlong,
) -> jstring {
    let dir = jstring_arg(&mut env, &dir);
    let instance_id = jstring_arg(&mut env, &instance_id);
    let json = widget_view::render_json(
        Path::new(&dir),
        &instance_id,
        width_dp,
        height_dp,
        now_from_millis(now_millis),
    );
    to_jstring(&mut env, &json)
}

/// `WidgetBridge.nativeConfigOptions(dir, instanceId)`: the placement activity's
/// account options JSON, seeding a fresh placement from the shared selection.
///
/// # Safety
/// See [`Java_tech_allaway_quotawidget_widget_WidgetBridge_nativeRender`].
#[no_mangle]
pub extern "system" fn Java_tech_allaway_quotawidget_widget_WidgetBridge_nativeConfigOptions(
    mut env: JNIEnv,
    _class: JClass,
    dir: JString,
    instance_id: JString,
) -> jstring {
    let dir = jstring_arg(&mut env, &dir);
    let instance_id = jstring_arg(&mut env, &instance_id);
    let json = widget_view::config_options_json(Path::new(&dir), &instance_id);
    to_jstring(&mut env, &json)
}

/// `WidgetBridge.nativeSaveInstance(dir, instanceId, json)`: persist one
/// instance's configuration. Returns an empty string on success, else the error
/// message.
///
/// # Safety
/// See [`Java_tech_allaway_quotawidget_widget_WidgetBridge_nativeRender`].
#[no_mangle]
pub extern "system" fn Java_tech_allaway_quotawidget_widget_WidgetBridge_nativeSaveInstance(
    mut env: JNIEnv,
    _class: JClass,
    dir: JString,
    instance_id: JString,
    json: JString,
) -> jstring {
    let dir = jstring_arg(&mut env, &dir);
    let instance_id = jstring_arg(&mut env, &instance_id);
    let json = jstring_arg(&mut env, &json);
    let result = widget_view::save_instance_json(Path::new(&dir), &instance_id, &json);
    to_jstring(&mut env, &result.err().unwrap_or_default())
}

/// `WidgetBridge.nativeRemoveInstance(dir, instanceId)`: forget an instance the
/// launcher removed. Returns an empty string on success, else the error.
///
/// # Safety
/// See [`Java_tech_allaway_quotawidget_widget_WidgetBridge_nativeRender`].
#[no_mangle]
pub extern "system" fn Java_tech_allaway_quotawidget_widget_WidgetBridge_nativeRemoveInstance(
    mut env: JNIEnv,
    _class: JClass,
    dir: JString,
    instance_id: JString,
) -> jstring {
    let dir = jstring_arg(&mut env, &dir);
    let instance_id = jstring_arg(&mut env, &instance_id);
    let result = widget_view::remove_instance(Path::new(&dir), &instance_id)
        .map_err(|e| e.to_string())
        .err()
        .unwrap_or_default();
    to_jstring(&mut env, &result)
}

/// `WidgetBridge.nativeRefresh(dir)`: the body of every durable refresh work
/// item — the one-time job the widget's refresh action and the app's manual
/// refresh enqueue, and the periodic ~15-minute schedule (issue #111). Performs
/// a real headless refresh — load config, decrypt the Keystore-backed secrets,
/// fetch every enabled account, and persist the read model — so a refresh
/// produces fresh data even with no activity open. Returns an empty string on
/// success, else the error.
///
/// The refresh itself is the shared `quota_core::refresh::refresh`; this
/// mirrors the foreground host's `refresh_once` (`mobile.rs`), and when this
/// process hosts the app its webview is told about the result the same way
/// (`mobile::app_handle`). That is exactly what ADR-0006 means by the native
/// scheduler owning refresh *opportunities* while the behaviour stays shared.
///
/// `context` is the worker's Android `Context` (its `applicationContext`), used
/// to reach the Keystore from this activity-less background process — see
/// [`headless_refresh`].
///
/// # Safety
/// See [`Java_tech_allaway_quotawidget_widget_WidgetBridge_nativeRender`].
#[no_mangle]
pub extern "system" fn Java_tech_allaway_quotawidget_widget_WidgetBridge_nativeRefresh(
    mut env: JNIEnv,
    _class: JClass,
    dir: JString,
    context: JObject,
) -> jstring {
    let dir = jstring_arg(&mut env, &dir);
    let result = headless_refresh(&mut env, &context, Path::new(&dir))
        .err()
        .unwrap_or_default();
    to_jstring(&mut env, &result)
}

/// Seed the Keystore-backed secret store from the worker's own Android context.
///
/// The foreground host registers the store from tao's *activity* context at
/// startup (`secrets::init_store`), but a WorkManager worker runs with no
/// activity, so that path is unavailable and the store would stay unregistered —
/// every credential read would then fail. Instead we seed `ndk_context` from the
/// worker's `applicationContext` (a real Context a background process does have)
/// and register the store, sharing the same one-time guard as the activity path.
///
/// The context pointer is stored by `ndk_context` for the store's lifetime, so
/// it must outlive this call: we take a global ref and deliberately leak it, so
/// it stays valid for the whole process (the application context is a
/// process-singleton anyway). This matches the process-lifetime validity tao
/// gives the activity ref.
fn init_worker_keystore(env: &mut JNIEnv, context: &JObject) -> Result<(), String> {
    let vm = env.get_java_vm().map_err(|e| e.to_string())?;
    let vm_ptr = vm.get_java_vm_pointer() as *mut std::ffi::c_void;
    let global = env.new_global_ref(context).map_err(|e| e.to_string())?;
    let ctx_ptr = global.as_raw() as *mut std::ffi::c_void;
    // Leak the global ref: `ndk_context` keeps the raw pointer, so the ref must
    // never be dropped for as long as the store can dereference it.
    std::mem::forget(global);
    // Safety: `vm_ptr` is the process JavaVM and `ctx_ptr` a leaked global ref
    // to the application context, both valid for the process lifetime.
    unsafe { crate::secrets::init_store_with_context(vm_ptr, ctx_ptr) }
}

/// One headless refresh pass over the persisted config and Keystore secrets,
/// persisting the resulting read model and alert memory. Kept close to
/// `mobile.rs::refresh_once` so the two never drift in what a refresh means.
fn headless_refresh(env: &mut JNIEnv, context: &JObject, dir: &Path) -> Result<(), String> {
    // This pass's attempt generation — allocated from the persisted monotonic
    // counter under the store lock, never the wall clock (concurrent starts
    // can collide and clock adjustments go backwards; see `next_generation`).
    // A failed allocation fails the whole pass: an unorderable result must not
    // be written, and WorkManager will retry.
    let attempt = quota_core::snapshots::next_generation(dir)
        .map_err(|e| format!("allocating refresh generation: {e}"))?;
    // The Keystore-backed store must be registered before the secret loader
    // below can decrypt any stored credential. If it cannot be — a
    // device/state where the worker's context init fails — we must NOT
    // proceed: a refresh with no decryptable secrets marks every account
    // "not configured"/failed and would overwrite the app's good read model
    // with an all-stale one (the "stale as of now" bug). Bail instead,
    // leaving snapshots.json intact so the widget keeps the last-known data,
    // and let WorkManager retry.
    init_worker_keystore(env, context)?;

    // Load the config for a headless refresh, which must never substitute the
    // built-in defaults: a corrupt or not-yet-written shared config would
    // otherwise fetch and persist a read model for accounts the user never
    // configured (Claude/Codex are enabled in `Config::default`). Both non-Present
    // outcomes stand down and leave snapshots.json intact so the widget keeps its
    // last-known data; WorkManager retries on the returned error. See
    // `Config::load_presence`.
    let cfg = match Config::load_presence(dir) {
        ConfigPresence::Present(cfg) => cfg,
        // Nothing is configured yet — no accounts, no credentials. Success with
        // no write; there is genuinely nothing to refresh.
        ConfigPresence::Absent => return Ok(()),
        ConfigPresence::Corrupt(recovery) => {
            return Err(format!(
                "shared configuration is unreadable; refusing to refresh over the built-in \
                 defaults and keeping the last-known read model: {}",
                recovery.message()
            ));
        }
    };
    // The error-reporting loader, not the plain one (issue #199): keys the
    // Keystore holds but cannot decrypt land in `failed` and ride the context
    // as `failed_secrets`, so the shared refresh reconciles them into an
    // explicit *unavailable* state — the same read the foreground app
    // produces — instead of the generic "not configured" a merely-absent
    // secret would yield. This is the change that closes the app/worker
    // divergence the issue was filed for.
    let (secrets, failed) = crate::secrets::load_all_reporting_errors(dir, &cfg);
    let mut ctx = ProviderCtx::new(dir.to_path_buf(), dir.to_path_buf(), secrets, cfg.clone());
    ctx.failed_secrets = failed;
    let dir_owned: PathBuf = dir.to_path_buf();
    ctx.on_secret_update = Some(Arc::new(move |key: &str, value: &str| {
        crate::secrets::set(&dir_owned, key, value)
    }));

    let prior = SnapshotStore::load(dir).prior_map();
    let mut engine = AlertEngine::load(dir);

    // A current-thread runtime is enough: this JNI call already runs on the
    // WorkManager worker thread, and the fetches inside `refresh` are I/O-bound
    // and joined concurrently on that one runtime.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let mut outcome = rt.block_on(quota_core::refresh::refresh(&ctx, &prior, &mut engine));

    // Persist alert memory (baselines and crossings) before dropping the
    // engine, so a later process measures against what this refresh saw.
    if let Err(e) = engine.save(dir) {
        return Err(format!("saving alert memory: {e}"));
    }

    // Deliver this pass's planned notifications (issue #112) before
    // `outcome.snapshots` is moved into the store below: quota-core has already
    // filtered the events through the per-account toast toggles and the
    // baseline rule and produced the full/public content pair, so all that
    // crosses the boundary here is the finished JSON. Posting is best-effort —
    // a failed delivery is logged and never fails the worker (which would only
    // retry a fetch the alert memory already accounts for). The foreground app
    // delivers its own plan the same way (`mobile.rs::refresh_once`); the two
    // hosts never run the same pass, and the engine's edge-triggering means a
    // crossing notifies once, not twice.
    let plan = quota_core::alerts::plan_notifications_json(&outcome, &cfg);
    if plan != "[]" {
        if let Err(e) = crate::android_notifications::deliver_with_env(env, context, &plan) {
            eprintln!("[worker] delivering notification plan failed: {e}");
        }
    }

    // Configured display order, then merge-and-store the read model every cold
    // widget reads. The merge matters here more than anywhere else: this worker
    // races the foreground app's own refreshes — in this process or, for a
    // widget-only cold start, in another one — and its fetches were composed
    // from the priors it loaded at start. A whole-file overwrite could regress
    // a success the app persisted while these fetches ran (or vice versa); the
    // generation-aware merge keeps the fresher observation per provider, and
    // the aggregate is folded over the merged list so the stored colour matches
    // the stored cards.
    cfg.sort_snapshots(&mut outcome.snapshots);
    let store = SnapshotStore::merge_and_store(dir, outcome.snapshots, attempt, &cfg)
        .map_err(|e| format!("storing snapshots: {e}"))?;

    // When the foreground app is alive in this process, its webview learns
    // about the new read model exactly as it does from `refresh_once`'s emit —
    // a manual refresh enqueued from the app (issue #111) and a periodic tick
    // that fires while the app is open both want the cards to move without
    // waiting for the next visibility change. In the widget-only process there
    // is no webview; persisting the read model is the whole delivery. The
    // published list is the merged truth (see above), never just this worker's
    // own outcome — and publication is gated on the newest applied generation
    // (see mobile.rs's PUBLISHED_GENERATION): a foreground pass that began
    // later may have already published, and this worker's older emit must not
    // regress what it delivered.
    if let Some(handle) = crate::mobile::app_handle() {
        if crate::mobile::publish_snapshots(handle, attempt, store.snapshots) {
            // The worker's provenance marker, on its own event: this path is
            // the only one that emits it (the foreground loop and entry
            // refresh go through `refresh_once`, which never does), so a
            // listener — and the emulator check's delivery assertion — can
            // attribute an update to the durable work itself rather than to
            // any refresh. It fires only when this worker's generation actually
            // won publication; payload is the read model's own write stamp.
            if let Err(e) = handle.emit("worker-refresh", &store.refreshed_at) {
                eprintln!("[worker] emitting the worker-refresh marker failed: {e}");
            }
        }
    }
    Ok(())
}
