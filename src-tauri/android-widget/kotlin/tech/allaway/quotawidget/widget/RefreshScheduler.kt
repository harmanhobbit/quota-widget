package tech.allaway.quotawidget.widget

import android.content.Context
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.PeriodicWorkRequest
import androidx.work.WorkManager
import java.util.concurrent.TimeUnit

/**
 * The background refresh schedule (issue #111, ADR-0006).
 *
 * WorkManager is the native host's scheduler, exactly as ADR-0006 assigns it:
 * a best-effort periodic refresh targeting [PERIODIC_MINUTES] minutes keeps the
 * persisted read model current while the app is not in use, with no permanent
 * foreground service. That number is a *target*, never a guarantee — WorkManager
 * defers under Doze, coalesces jobs and honours no exact time — so every surface
 * describing it says "roughly every 15 minutes", never "every 15 minutes" (see
 * MobileApp.svelte's "Background refresh" section and
 * `quota_core::refresh::BACKGROUND_REFRESH_TARGET` on the Rust side, which this
 * must agree with; [RefreshSchedulerTest] guards that agreement).
 *
 * The same worker also serves the two non-periodic entry points into background
 * refresh: a widget's refresh action enqueues [enqueueOneTime] work directly,
 * and the app's manual refresh button reaches [enqueueOneTime] through the JNI
 * call in `src-tauri/src/android_schedule.rs` — one durable unit of work for
 * every refresh nobody is watching.
 *
 * Both entry points are `@JvmStatic` on purpose: `android_schedule.rs` reaches
 * them with JNI `CallStaticVoidMethod` on this class. A Kotlin `object`'s plain
 * members live on the singleton instance only — without `@JvmStatic` there is
 * no static method for `GetStaticMethodID` to find, the call fails with
 * `NoSuchMethodError`, and (because that call is deliberately best-effort) the
 * schedule would silently never exist. The emulator check asserts a real
 * periodic job exists precisely so that failure mode stays impossible.
 */
object RefreshScheduler {
    /**
     * The periodic job's unique name. One schedule per app: the app and every
     * widget instance render the same persisted read model, so a single periodic
     * refresh keeps all of them current.
     */
    const val PERIODIC_WORK = "quota-widget-periodic-refresh"

    /**
     * The background refresh *target*, in minutes. WorkManager's periodic
     * minimum is 15 minutes, which is why the target is 15 — a smaller request
     * would be a lie the scheduler silently rounds up anyway.
     */
    const val PERIODIC_MINUTES = 15L

    /**
     * Make sure the periodic refresh exists. Idempotent: unique work with
     * [periodicPolicy], so calling this on every app start and every widget
     * update never resets the run clock — KEEP leaves an existing schedule's
     * next run time alone, where UPDATE would postpone it to a full interval
     * from *now* and thus penalise the user for opening the app.
     */
    @JvmStatic
    fun ensurePeriodic(context: Context) {
        WorkManager.getInstance(context).enqueueUniquePeriodicWork(
            PERIODIC_WORK,
            periodicPolicy(),
            periodicRequest(),
        )
    }

    /**
     * The policy [ensurePeriodic] enqueues under. Extracted (like
     * [periodicRequest]) so the JVM tests pin the scheduling configuration —
     * KEEP is load-bearing and must be changed deliberately, never absorbed
     * into a routine refactor of the call site.
     */
    fun periodicPolicy(): ExistingPeriodicWorkPolicy = ExistingPeriodicWorkPolicy.KEEP

    /**
     * The app's manual refresh: one-time durable work that can finish
     * independently of the activity that requested it (issue #111) — the same
     * unique work the widget's refresh action enqueues. REPLACE means bursts
     * from either surface never stack (a pending request is replaced; a
     * running one is cancelled and restarted); it does not merge them into
     * one fetch.
     */
    @JvmStatic
    fun enqueueOneTime(context: Context) {
        WidgetRefreshWorker.enqueue(context)
    }

    /**
     * The periodic request [ensurePeriodic] enqueues, built from the documented
     * target and tagged with the schedule's unique name. Extracted so the JVM
     * tests can assert the actual scheduling configuration — interval, worker,
     * tag, and the deliberate absence of constraints — rather than trusting the
     * call site to mean what it says.
     */
    fun periodicRequest(): PeriodicWorkRequest =
        PeriodicWorkRequestBuilder<WidgetRefreshWorker>(
            PERIODIC_MINUTES,
            TimeUnit.MINUTES,
        )
            .addTag(PERIODIC_WORK)
            .build()
}
