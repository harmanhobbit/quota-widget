package tech.allaway.quotawidget.widget

import android.appwidget.AppWidgetManager
import android.content.Context
import androidx.glance.GlanceId
import androidx.glance.action.ActionParameters
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.action.ActionCallback

/**
 * The launcher-facing broadcast receiver for the home-screen widget. Glance
 * hosts the actual rendering in [QuotaGlanceWidget]; this only wires it to the
 * `AppWidgetProvider` the manifest registers.
 */
class QuotaWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget: GlanceAppWidget = QuotaGlanceWidget()

    /**
     * A widget placement (or any system widget update) is a refresh opportunity
     * even if the app has not been opened since boot (issue #111): make sure the
     * best-effort periodic refresh exists. Unique work with KEEP — idempotent,
     * and it never disturbs a schedule that already runs.
     */
    override fun onUpdate(
        context: Context,
        appWidgetManager: AppWidgetManager,
        appWidgetIds: IntArray,
    ) {
        super.onUpdate(context, appWidgetManager, appWidgetIds)
        RefreshScheduler.ensurePeriodic(context)
    }
}

/**
 * The refresh tap's action. It does not fetch inline — a receiver/callback is
 * short-lived — it enqueues the one-time durable [WidgetRefreshWorker], which
 * runs to completion after this callback returns (issue #113).
 */
class RefreshAction : ActionCallback {
    override suspend fun onAction(
        context: Context,
        glanceId: GlanceId,
        parameters: ActionParameters,
    ) {
        WidgetRefreshWorker.enqueue(context)
    }
}
