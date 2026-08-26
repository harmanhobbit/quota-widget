package tech.allaway.quotawidget.widget

import android.content.Context
import androidx.glance.appwidget.updateAll
import androidx.work.CoroutineWorker
import androidx.work.ExistingWorkPolicy
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkerParameters
import androidx.work.WorkManager

/**
 * The one-time durable refresh work a widget's refresh action enqueues
 * (issue #113: "widget refresh enqueues one-time durable work — not the
 * short-lived receiver").
 *
 * Glance action callbacks and broadcast receivers run on a short leash — the
 * system may kill the receiver as soon as the callback returns, long before a
 * network refresh finishes. So the refresh action does not fetch inline; it
 * enqueues this worker, which WorkManager runs to completion on its own thread,
 * surviving the receiver's exit. The worker calls the shared headless refresh
 * (`WidgetBridge.nativeRefresh` → `quota_core::refresh`), then asks every placed
 * widget to re-read the freshly persisted read model.
 *
 * The ~15-minute periodic scheduling of the same refresh is issue #111's; this
 * is the manual, on-demand unit of durable work #113 requires.
 */
class WidgetRefreshWorker(
    context: Context,
    params: WorkerParameters,
) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result {
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
        private const val UNIQUE_WORK = "quota-widget-refresh"

        /**
         * Enqueue a single refresh, replacing any already pending so a burst of
         * taps collapses to one fetch. Unique + durable: it outlives the caller.
         */
        fun enqueue(context: Context) {
            val request = OneTimeWorkRequestBuilder<WidgetRefreshWorker>().build()
            WorkManager.getInstance(context)
                .enqueueUniqueWork(UNIQUE_WORK, ExistingWorkPolicy.REPLACE, request)
        }
    }
}
