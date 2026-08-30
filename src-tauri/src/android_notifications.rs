//! Rust → JVM calls that hand a refresh's notification plan to the native host
//! (issue #112).
//!
//! ADR-0006 gives notification *posting* to the native Android host, so the
//! Rust side never touches `NotificationManager` directly: the plan is decided
//! entirely in `quota-core` (`alerts::plan_notifications` — the presentation
//! filters plus the full/public content pair) and this module only marshals the
//! resulting JSON to Kotlin (`AlertNotifier.deliver`), the mirror image of
//! `widget_jni.rs`, where Kotlin calls into Rust. Both directions obey the same
//! rule: the boundary is mechanical marshalling, and every real decision stays
//! on the side that owns it.
//!
//! Two entry points, one per refresh host, both landing on the same Kotlin
//! method:
//!
//! - [`deliver`] — the foreground app's `mobile.rs::refresh_once`, which has no
//!   `JNIEnv` of its own and reaches the JVM through `ndk_context` via
//!   [`with_app_env`].
//! - [`deliver_with_env`] — the WorkManager worker's `widget_jni.rs::
//!   headless_refresh`, which is already inside a JNI call and holds the
//!   worker's `JNIEnv` and `applicationContext`.
//!
//! Every call is best-effort: a failure is returned (and logged by the caller)
//! rather than panicking, because a failed *notification* must never take a
//! refresh down — the read model it accompanies is the important delivery.

use jni::objects::{JClass, JObject, JString, JValue};
use jni::JavaVM;

/// The `AlertNotifier` class the Kotlin host compiles into the app.
const NOTIFIER_CLASS: &str = "tech.allaway.quotawidget.widget.AlertNotifier";

/// `AlertNotifier.deliver(Context, String)`.
const DELIVER_SIG: &str = "(Landroid/content/Context;Ljava/lang/String;)V";

/// Hand one refresh's notification plan (`alerts::plan_notifications_json`) to
/// the Kotlin host for posting. Foreground path: the plan for the pass the
/// visible app itself just completed.
pub fn deliver(plan_json: &str) -> Result<(), String> {
    with_app_env("deliver failed", |env, context| {
        call_deliver(env, context, plan_json)
    })
}

/// Same handoff for the headless worker path (`widget_jni.rs::headless_refresh`),
/// which is already inside a JNI call: `env` is the worker's thread and
/// `context` its `applicationContext` (the same object the worker handed Rust
/// for the Keystore).
pub fn deliver_with_env(
    env: &mut jni::JNIEnv,
    context: &JObject,
    plan_json: &str,
) -> Result<(), String> {
    let result = call_deliver(env, context, plan_json);
    if result.is_err() {
        // A JNI error generally means a Java exception is pending; leaving it
        // set aborts the VM at the next unrelated JNI call. Drop it — the
        // error string below is the report.
        let _ = env.exception_clear();
    }
    result.map_err(|e| format!("deliver failed: {e}"))
}

/// The platform's view of the notification permission (issue #112): `"granted"`
/// or `"denied"` on Android 13+, and `"granted"` below 13 where the permission
/// does not exist (notifications there are on by default and controlled only
/// by the system-settings toggle, which the state read already reflects).
/// Feeds the mobile Settings row; not a decision by itself.
pub fn permission_state() -> Result<String, String> {
    with_app_env("permission state failed", |env, context| {
        let class = resolve_class(env, context, NOTIFIER_ACCESS_CLASS)?;
        let value = env
            .call_static_method(
                class,
                "state",
                "(Landroid/content/Context;)Ljava/lang/String;",
                &[JValue::Object(context)],
            )?
            .l()?;
        let jstr = JString::from(value);
        Ok(env
            .get_string(&jstr)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default())
    })
}

/// Fire the one POST_NOTIFICATIONS runtime request from the foreground
/// Activity. Called from `mobile.rs`'s `request_notification_permission`
/// command, i.e. only while the app is open and in response to the contextual
/// prompt (first successful account + notifications wanted) — never from the
/// background worker, where a permission dialog is not allowed and would fail.
///
/// The context `ndk_context` holds is handed to Kotlin as-is:
/// `NotificationAccess.request` walks the `ContextWrapper` chain to find the
/// `Activity` it needs and returns whether the dialog actually started. Rust
/// must not cast instead: JNI verifies no argument types, so handing a
/// non-Activity where the signature says `Activity` would die in the callee's
/// `checkcast` (a crash), not in this call. `Ok` is returned only when Kotlin
/// reports the request started (below Android 13 it always does — there is
/// nothing to request and the state is permanently `"granted"`), which is the
/// signal `mobile.rs` relies on before burning the durable one-shot: a request
/// that never started must not be recorded as issued.
pub fn request_from_activity() -> Result<(), String> {
    let started = with_app_env("permission request failed", |env, context| {
        let class = resolve_class(env, context, NOTIFIER_ACCESS_CLASS)?;
        let started = env
            .call_static_method(
                class,
                "request",
                "(Landroid/content/Context;)Z",
                &[JValue::Object(context)],
            )?
            .z()?;
        Ok(started)
    })?;
    if started {
        Ok(())
    } else {
        Err(
            "permission request failed: no Activity is reachable from the foreground \
             context, so the system dialog was not started"
                .into(),
        )
    }
}

/// Open the system's notification settings for this app (`ACTION_APP_
/// NOTIFICATION_SETTINGS` with the package pinned). The no-re-prompt recovery
/// path: after the one-time request is spent, Settings is how notifications
/// come back on.
pub fn open_settings() -> Result<(), String> {
    with_app_env("open notification settings failed", |env, context| {
        let class = resolve_class(env, context, NOTIFIER_ACCESS_CLASS)?;
        env.call_static_method(
            class,
            "openSettings",
            "(Landroid/content/Context;)V",
            &[JValue::Object(context)],
        )?;
        Ok(())
    })
}

/// The `NotificationAccess` class the Kotlin host compiles into the app: the
/// permission half of the notification seam (state/request/settings), kept
/// separate from `AlertNotifier`'s posting so the worker-only code paths never
/// touch Activity APIs.
const NOTIFIER_ACCESS_CLASS: &str = "tech.allaway.quotawidget.widget.NotificationAccess";

/// Run `f` against the foreground host: the fenced `ndk_context` lookup, the
/// thread attach and the activity-context borrow all happen here, and the
/// attach guard never escapes this function — it cannot. `attach_current_thread`
/// yields an `AttachGuard` that borrows the local `JavaVM` (and derefs to
/// `JNIEnv` only through it), so acquisition and use must be one stretch: the
/// inline shape `android_schedule::call_scheduler` established, factored once
/// instead of repeated per entry point.
///
/// Every entry point that does not already hold a `JNIEnv` goes through this
/// (i.e. everything except [`deliver_with_env`]). A pending Java exception left
/// by a failing `f` is cleared before returning, so the boundary can never
/// poison the surrounding runtime; a JNI error is reported as `{what}: {e}`.
fn with_app_env<T>(
    what: &str,
    f: impl for<'env> FnOnce(&mut jni::JNIEnv<'env>, &JObject<'env>) -> Result<T, jni::errors::Error>,
) -> Result<T, String> {
    // `ndk_context::android_context()` panics rather than erroring when the
    // context is not (or no longer) registered — tao removes it when the
    // activity is destroyed. A notification call that somehow raced that
    // destruction must surface as a failed call, never a crash, so the lookup
    // is fenced.
    let ctx = std::panic::catch_unwind(std::panic::AssertUnwindSafe(ndk_context::android_context))
        .map_err(|_| {
            "no Android context registered — the activity is not running, so there is \
             nothing to deliver a notification from"
                .to_string()
        })?;
    // Safety: `ctx.vm()` is the process JavaVM pointer registered by
    // `secrets::init_store` at startup and valid for the process lifetime.
    let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    // Safety: `ctx.context()` is tao's GlobalRef to the activity, alive for as
    // long as this process. `from_raw` does not take ownership, so nothing is
    // deleted behind tao's back.
    let context = unsafe { JObject::from_raw(ctx.context().cast()) };

    let result = f(&mut env, &context);
    if result.is_err() {
        // A JNI error generally means a Java exception is pending; leaving it
        // set aborts the VM at the next unrelated JNI call (observed as
        // "JNI DETECTED ERROR IN APPLICATION: ... called with pending
        // exception"). Drop it — the error string above is the report.
        let _ = env.exception_clear();
    }
    result.map_err(|e| format!("{what}: {e}"))
}

/// Resolve an APK class through the *context's* classloader and call
/// `deliver(Context, String)` on it, returning any JNI error to the caller,
/// which owns the exception hygiene.
///
/// The class must be resolved through the app's classloader, not via
/// `find_class`: on a thread attached bare (the foreground path above) JNI's
/// FindClass falls back to the system classloader, which cannot see anything in
/// the APK — the failure `android_schedule.rs` first observed as a pending
/// `ClassNotFoundException` aborting the app. Using the context's loader in the
/// worker path too keeps one exception-hygiene shape for both.
fn call_deliver(
    env: &mut jni::JNIEnv<'_>,
    context: &JObject<'_>,
    plan_json: &str,
) -> Result<(), jni::errors::Error> {
    let class = resolve_class(env, context, NOTIFIER_CLASS)?;
    let plan: JString = env.new_string(plan_json)?;
    env.call_static_method(
        class,
        "deliver",
        DELIVER_SIG,
        // The plan travels as a Java string; Kotlin parses it with org.json.
        &[JValue::Object(context), (&plan).into()],
    )?;
    Ok(())
}

/// Resolve an app class by its binary name through the given context's
/// classloader (see the classloader note on [`call_deliver`]). The caller owns
/// clearing any pending exception on failure.
fn resolve_class<'local>(
    env: &mut jni::JNIEnv<'local>,
    context: &JObject,
    name: &str,
) -> Result<JClass<'local>, jni::errors::Error> {
    let loader = env
        .call_method(context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?
        .l()?;
    let class_name: JString = env.new_string(name)?;
    let class = env
        .call_method(
            &loader,
            "loadClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[(&class_name).into()],
        )?
        .l()?;
    Ok(JClass::from(class))
}
