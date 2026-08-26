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
//! - `nativeRefresh` is the body of the one-time durable WorkManager job the
//!   widget's refresh action enqueues: it performs a real headless refresh
//!   (load config, decrypt Keystore secrets, fetch, persist the read model) so
//!   the work finishes after the short-lived broadcast receiver has exited.
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
use jni::objects::{JClass, JString};
use jni::sys::{jdouble, jlong, jstring};
use jni::JNIEnv;

use quota_core::alerts::AlertEngine;
use quota_core::config::Config;
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

/// `WidgetBridge.nativeRefresh(dir)`: the body of the widget's one-time durable
/// WorkManager job. Performs a real headless refresh — load config, decrypt the
/// Keystore-backed secrets, fetch every enabled account, and persist the read
/// model — so a widget's manual refresh produces fresh data even with no
/// activity open. Returns an empty string on success, else the error.
///
/// The refresh itself is the shared `quota_core::refresh::refresh`; this mirrors
/// the foreground host's `refresh_once` (`mobile.rs`) minus the webview emit,
/// which is exactly what ADR-0006 means by the native scheduler owning refresh
/// opportunities while the behaviour stays shared. The periodic ~15-minute
/// scheduling of this same work is issue #111's; this entry point is the unit
/// of durable work #113's refresh action enqueues.
///
/// # Safety
/// See [`Java_tech_allaway_quotawidget_widget_WidgetBridge_nativeRender`].
#[no_mangle]
pub extern "system" fn Java_tech_allaway_quotawidget_widget_WidgetBridge_nativeRefresh(
    mut env: JNIEnv,
    _class: JClass,
    dir: JString,
) -> jstring {
    let dir = jstring_arg(&mut env, &dir);
    let result = headless_refresh(Path::new(&dir)).err().unwrap_or_default();
    to_jstring(&mut env, &result)
}

/// One headless refresh pass over the persisted config and Keystore secrets,
/// persisting the resulting read model and alert memory. Kept close to
/// `mobile.rs::refresh_once` so the two never drift in what a refresh means.
fn headless_refresh(dir: &Path) -> Result<(), String> {
    // The Keystore-backed store must be registered before `load_all` can
    // decrypt any stored credential — the same one-time init the foreground
    // host does at startup (`secrets::init_store`). Ignoring an error here is
    // deliberate: a device with no stored secrets still refreshes (every
    // account simply reports "not configured"), which is a truthful widget
    // state, not a crash.
    let _ = crate::secrets::init_store();

    let cfg = Config::load(dir).config;
    let secrets = crate::secrets::load_all(dir, &cfg);
    let mut ctx = ProviderCtx::new(dir.to_path_buf(), dir.to_path_buf(), secrets, cfg.clone());
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

    // Configured display order, then persist the read model every cold widget
    // reads. The aggregate is folded once here so the widget need not recompute
    // it (matching `mobile.rs`).
    cfg.sort_snapshots(&mut outcome.snapshots);
    let aggregate = quota_core::refresh::aggregate_status(&outcome.snapshots, &cfg);
    let store = SnapshotStore::from_snapshots(outcome.snapshots, aggregate);
    store
        .save(dir)
        .map_err(|e| format!("saving snapshots: {e}"))
}
