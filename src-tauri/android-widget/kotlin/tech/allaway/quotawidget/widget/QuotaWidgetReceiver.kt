package tech.allaway.quotawidget.widget

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
