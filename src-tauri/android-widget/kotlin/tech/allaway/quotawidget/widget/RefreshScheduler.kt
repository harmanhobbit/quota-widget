package tech.allaway.quotawidget.widget

import android.content.Context
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.PeriodicWorkRequestBuilder
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
     * [ExistingPeriodicWorkPolicy.KEEP], so calling this on every app start and
     * every widget update never resets the run clock — KEEP leaves an existing
     * schedule's next run time alone, where UPDATE would postpone it to a full
     * interval from *now* and thus penalise the user for opening the app.
     */
    fun ensurePeriodic(context: Context) {
        val request = PeriodicWorkRequestBuilder<WidgetRefreshWorker>(
            PERIODIC_MINUTES,
            TimeUnit.MINUTES,
        ).build()
        WorkManager.getInstance(context).enqueueUniquePeriodicWork(
            PERIODIC_WORK,
            ExistingPeriodicWorkPolicy.KEEP,
            request,
        )
    }

    /**
     * The app's manual refresh: one-time durable work that can finish
     * independently of the activity that requested it (issue #111) — the same
     * unique work the widget's refresh action enqueues, so a burst of taps from
     * either surface collapses to one fetch.
     */
    fun enqueueOneTime(context: Context) {
        WidgetRefreshWorker.enqueue(context)
    }
}
