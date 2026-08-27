//! Rust → JVM calls into the native host's WorkManager scheduler (issue #111).
//!
//! ADR-0006 gives WorkManager to the native Android host, so the Rust side
//! never talks to WorkManager directly: the schedule lives in Kotlin
//! (`src-tauri/android-widget/kotlin/.../widget/RefreshScheduler.kt`), and this
//! module only marshals two calls across the JNI boundary — the mirror image of
//! `widget_jni.rs`, where Kotlin calls into Rust. Both directions obey the same
//! rule: the boundary is mechanical marshalling, and every real decision stays
//! on the side that owns it.
//!
//! The JVM connection comes from `ndk_context`, which `secrets::init_store`
//! initializes from tao's activity context during Tauri's setup — by the time
//! anything here runs, the context is registered and the Keystore store is up.
//! Every call is best-effort: a failure is returned (and logged by the caller)
//! rather than panicking, because a failed *enqueue* must never take the app
//! down — the foreground refresh still works without any of this.

use jni::objects::{JClass, JObject, JString, JValue};
use jni::JavaVM;

/// The `RefreshScheduler` class the Kotlin host compiles into the app.
const SCHEDULER_CLASS: &str = "tech.allaway.quotawidget.widget.RefreshScheduler";

/// Call `RefreshScheduler.<method>(Context)` on the JVM, passing the context
/// `ndk_context` holds (tao's activity reference, valid for the app's lifetime;
/// WorkManager itself only needs it to reach the application).
fn call_scheduler(method: &str) -> Result<(), String> {
    // `ndk_context::android_context()` panics rather than erroring when the
    // context is not (or no longer) registered — tao removes it when the
    // activity is destroyed. A scheduling call that somehow raced that
    // destruction must surface as a failed enqueue, never a crash, so the
    // lookup is fenced.
    let ctx = std::panic::catch_unwind(std::panic::AssertUnwindSafe(ndk_context::android_context))
        .map_err(|_| {
            "no Android context registered — the activity is not running, so there is \
             nothing to schedule work from"
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

    // The class must be resolved through the *app's* classloader, reached off
    // the context, not via `find_class`: this thread was attached bare (no
    // defining Java class), so JNI's FindClass falls back to the system
    // classloader, whose DexPathList is just `.` — it cannot see anything in
    // the APK. The first dispatch of this code crashed the whole app with a
    // pending ClassNotFoundException aborting at the next JNI call, which is
    // the failure mode the exception hygiene below exists to keep impossible:
    // whatever goes wrong here, this function returns with no exception left
    // pending, so the scheduler can never poison the surrounding runtime.
    let result = (|| {
        let loader = env
            .call_method(&context, "getClassLoader", "()Ljava/lang/ClassLoader;", &[])?
            .l()?;
        let name: JString = env.new_string(SCHEDULER_CLASS)?;
        let class = env
            .call_method(
                &loader,
                "loadClass",
                "(Ljava/lang/String;)Ljava/lang/Class;",
                &[(&name).into()],
            )?
            .l()?;
        env.call_static_method(
            JClass::from(class),
            method,
            "(Landroid/content/Context;)V",
            &[JValue::Object(&context)],
        )?;
        Ok(())
    })()
    .map_err(|e: jni::errors::Error| format!("{method} failed: {e}"));
    if result.is_err() {
        // A JNI error generally means a Java exception is pending; leaving it
        // set aborts the VM at the next unrelated JNI call (observed as
        // "JNI DETECTED ERROR IN APPLICATION: ... called with pending
        // exception"). Drop it — the error string above is the report.
        let _ = env.exception_clear();
    }
    result
}

/// Make sure the best-effort periodic refresh (~15-minute target) exists.
/// Idempotent on the Kotlin side (unique periodic work, KEEP policy), so this
/// runs at every app start without ever resetting the schedule's clock.
pub fn ensure_periodic() -> Result<(), String> {
    call_scheduler("ensurePeriodic")
}

/// Enqueue the app's manual refresh as one-time durable work (issue #111): the
/// fetch then completes under WorkManager even if the activity is dismissed
/// immediately after the tap. Shares the widget refresh action's unique
/// one-time work, so overlapping requests collapse into a single fetch.
pub fn enqueue_manual_refresh() -> Result<(), String> {
    call_scheduler("enqueueOneTime")
}
