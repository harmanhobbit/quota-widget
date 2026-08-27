package tech.allaway.quotawidget.widget

import android.content.Context
import android.util.Log
import androidx.glance.appwidget.updateAll
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.OneTimeWorkRequest
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters

/**
 * The one-time durable refresh work (issue #113), and the unit every schedule
 * is built from (issue #111): a widget's refresh action enqueues it, the app's
 * manual refresh enqueues it through [RefreshScheduler.enqueueOneTime], and the
 * periodic ~15-minute schedule runs it on the same unique name.
 *
 * Glance action callbacks and broadcast receivers run on a short leash — the
 * system may kill the receiver as soon as the callback returns, long before a
 * network refresh finishes. So no fetch happens inline; the caller enqueues
 * this worker, which WorkManager runs to completion on its own thread,
 * surviving the receiver's exit. The worker calls the shared headless refresh
 * (`WidgetBridge.nativeRefresh` → `quota_core::refresh`), then asks every placed
 * widget to re-read the freshly persisted read model.
 */
class WidgetRefreshWorker(
    context: Context,
    params: WorkerParameters,
) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
        // WorkManager executes this worker only for a persisted request, so
        // this line (with the request's tags) is the observable that attributes
        // a run to its entry point — manual/widget taps carry [TAG_MANUAL], the
        // periodic schedule carries [RefreshScheduler.PERIODIC_WORK]. The
        // emulator check asserts on it: a log line is cumulative history, so a
        // fast-failing fetch cannot fall between samples the way a JobScheduler
        // poll can.
        Log.i(TAG, "durable refresh running; tags=$tags")
        val error = WidgetBridge.nativeRefresh(
            WidgetPaths.configDir(applicationContext),
            applicationContext,
        )
        // Re-render every instance whether or not the fetch succeeded: a failed
        // refresh still updates data age and any newly-stale rows, and the read
        // model keeps the last-known readings (ADR-0006).
        QuotaGlanceWidget().updateAll(applicationContext)
        return if (error.isEmpty()) Result.success() else Result.retry()
    }

    companion object {
        private const val TAG = "QuotaWidgetRefresh"
        private const val UNIQUE_WORK = "quota-widget-refresh"

        /**
         * Unique tag identifying the manual/on-demand request — the unit the
         * widget's refresh action and the app's manual refresh button both
         * enqueue. WorkManager does not surface request tags to JobScheduler's
         * dump, but the worker logs them at execution time (see [doWork]), and
         * the JVM tests pin them on the request itself.
         */
        const val TAG_MANUAL = "quota-widget-manual-refresh"

        /**
         * Enqueue a single refresh, replacing any already pending so a burst of
         * taps collapses to one fetch. Unique + durable: it outlives the caller.
         */
        fun enqueue(context: Context) {
            WorkManager.getInstance(context)
                .enqueueUniqueWork(UNIQUE_WORK, ExistingWorkPolicy.REPLACE, manualRequest())
        }

        /**
         * The manual request [enqueue] enqueues, built with [TAG_MANUAL].
         * Extracted so the JVM tests can assert the request's identity rather
         * than trusting the call site to mean what it says.
         */
        fun manualRequest(): OneTimeWorkRequest =
            OneTimeWorkRequestBuilder<WidgetRefreshWorker>()
                .addTag(TAG_MANUAL)
                .build()
    }
}
