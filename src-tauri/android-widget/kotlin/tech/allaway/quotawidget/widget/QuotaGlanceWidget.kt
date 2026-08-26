package tech.allaway.quotawidget.widget

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.GlanceTheme
import androidx.glance.LocalContext
import androidx.glance.LocalSize
import androidx.glance.action.clickable
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetManager
import androidx.glance.appwidget.SizeMode
import androidx.glance.appwidget.LinearProgressIndicator
import androidx.glance.appwidget.action.actionRunCallback
import androidx.glance.appwidget.action.actionStartActivity
import androidx.glance.appwidget.cornerRadius
import androidx.glance.appwidget.provideContent
import androidx.glance.background
import androidx.glance.layout.Alignment
import androidx.glance.layout.Column
import androidx.glance.layout.Row
import androidx.glance.layout.Spacer
import androidx.glance.layout.fillMaxWidth
import androidx.glance.layout.height
import androidx.glance.layout.padding
import androidx.glance.layout.size
import androidx.glance.text.FontWeight
import androidx.glance.text.Text
import androidx.glance.text.TextStyle
import androidx.glance.unit.ColorProvider
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * The home-screen widget (issue #113). It renders purely from the persisted
 * read model, projected and flattened by the shared library — no fetching, no
 * credentials, no quota decision re-made in Kotlin (ADR-0006). The launcher's
 * current cell dimensions choose the tier; the same dimensions are handed to the
 * shared projection, so what is drawn always matches what fits.
 */
class QuotaGlanceWidget : GlanceAppWidget() {
    // Exact: react to the launcher's real cell size, which is what the shared
    // breakpoint logic expects (both axes must clear a tier's floor).
    override val sizeMode = SizeMode.Exact

    override suspend fun provideGlance(context: Context, id: GlanceId) {
        val appWidgetId = GlanceAppWidgetManager(context).getAppWidgetId(id)
        provideContent {
            GlanceTheme {
                val size = LocalSize.current
                val ctx = LocalContext.current
                // Re-read only when the size changes; a manual/scheduled refresh
                // re-renders the whole widget, re-running this with fresh data.
                val view = remember(size.width, size.height, appWidgetId) {
                    parseWidgetView(
                        WidgetBridge.nativeRender(
                            WidgetPaths.configDir(ctx),
                            appWidgetId.toString(),
                            size.width.value.toDouble(),
                            size.height.value.toDouble(),
                            System.currentTimeMillis(),
                        )
                    )
                }
                WidgetSurface(appWidgetId, view)
            }
        }
    }
}

/** The status → colour map. Colours stay stable across light/dark; the tier's
 *  text/background follow the system theme via [GlanceTheme]. */
private fun statusColor(status: WidgetStatus): ColorProvider = when (status) {
    WidgetStatus.OK -> ColorProvider(Color(0xFF2E7D32), Color(0xFF66BB6A))
    WidgetStatus.WARN -> ColorProvider(Color(0xFFF9A825), Color(0xFFFFCA28))
    WidgetStatus.CRITICAL -> ColorProvider(Color(0xFFC62828), Color(0xFFEF5350))
    WidgetStatus.STALE -> ColorProvider(Color(0xFF616161), Color(0xFF9E9E9E))
    WidgetStatus.UNKNOWN -> ColorProvider(Color(0xFF616161), Color(0xFF9E9E9E))
}

@Composable
private fun WidgetSurface(appWidgetId: Int, view: WidgetView) {
    Column(
        modifier = GlanceModifier
            .fillMaxWidth()
            .background(GlanceTheme.colors.widgetBackground)
            .cornerRadius(16.dp)
            .padding(12.dp),
    ) {
        when (view.state) {
            WidgetState.NEEDS_CONFIGURATION ->
                Placeholder("Widget needs configuration", appWidgetId)
            WidgetState.NO_DATA ->
                Placeholder("No data—tap to refresh", appWidgetId, refresh = true)
            WidgetState.UNKNOWN ->
                Placeholder("Widget unavailable", appWidgetId)
            WidgetState.CONTENT -> view.content?.let { Content(view.size, it, appWidgetId) }
        }
    }
}

@Composable
private fun Placeholder(message: String, appWidgetId: Int, refresh: Boolean = false) {
    val modifier = if (refresh) {
        GlanceModifier.clickable(actionRunCallback<RefreshAction>())
    } else {
        GlanceModifier.clickable(actionStartActivity(configIntent(LocalContext.current, appWidgetId)))
    }
    Text(message, modifier = modifier, style = TextStyle(color = GlanceTheme.colors.onSurface))
}

@Composable
private fun Content(size: String, content: WidgetContent, appWidgetId: Int) {
    // Header: the shared aggregate cue (colour + non-colour label) and age.
    Row(verticalAlignment = Alignment.CenterVertically, modifier = GlanceModifier.fillMaxWidth()) {
        StatusDot(content.aggregateStatus)
        Spacer(GlanceModifier.size(6.dp))
        Text(
            content.aggregateStatus.label,
            style = TextStyle(
                color = statusColor(content.aggregateStatus),
                fontWeight = FontWeight.Bold,
            ),
        )
        Spacer(GlanceModifier.defaultWeight())
        RefreshButton()
    }
    content.dataAgeSecs?.let {
        Text(
            "as of ${formatAge(it)}",
            style = TextStyle(color = GlanceTheme.colors.onSurfaceVariant, fontSize = 11.sp),
        )
    }
    Spacer(GlanceModifier.size(8.dp))

    when (size) {
        "small" -> content.worst?.let { WorstHeadlineView(it) }
        else -> content.rows.forEach { row ->
            RowView(row, large = size == "large", appWidgetId = appWidgetId)
        }
    }
}

@Composable
private fun WorstHeadlineView(worst: WorstHeadline) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        StatusDot(worst.status)
        Spacer(GlanceModifier.size(6.dp))
        Column {
            Text(worst.name, style = TextStyle(color = GlanceTheme.colors.onSurface))
            CellText(worst.cell)
        }
    }
}

@Composable
private fun RowView(row: WidgetRow, large: Boolean, appWidgetId: Int) {
    val clickAction = if (row.removed) {
        // Removed account: tap to configure, never a substitute.
        actionStartActivity(configIntent(LocalContext.current, appWidgetId))
    } else {
        // Deep-link into the app at this account.
        actionStartActivity(accountIntent(LocalContext.current, row.providerId))
    }
    Column(modifier = GlanceModifier.fillMaxWidth().clickable(clickAction).padding(vertical = 4.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = GlanceModifier.fillMaxWidth()) {
            StatusDot(row.status ?: WidgetStatus.STALE)
            Spacer(GlanceModifier.size(6.dp))
            Text(row.name, style = TextStyle(color = GlanceTheme.colors.onSurface))
            Spacer(GlanceModifier.defaultWeight())
            if (row.removed) {
                Text(
                    "Account removed—tap to configure",
                    style = TextStyle(color = GlanceTheme.colors.onSurfaceVariant, fontSize = 11.sp),
                )
            }
        }
        row.cells.forEach { cell ->
            CellText(cell)
            if (large && cell.bar != null) {
                UsageBar(cell.bar)
            }
            if (large && cell.resetsAtSecs != null) {
                Text(
                    "resets ${formatReset(cell.resetsAtSecs)}",
                    style = TextStyle(color = GlanceTheme.colors.onSurfaceVariant, fontSize = 11.sp),
                )
            }
        }
    }
}

@Composable
private fun CellText(cell: HeadlineCell) {
    // Privacy redaction: the label stays, the figure becomes a dotted mask.
    val figure = cell.value ?: "•••"
    Text(
        "${cell.label}: $figure",
        style = TextStyle(color = GlanceTheme.colors.onSurface),
    )
}

@Composable
private fun UsageBar(fraction: Double) {
    LinearProgressIndicator(
        progress = fraction.coerceIn(0.0, 1.0).toFloat(),
        modifier = GlanceModifier.fillMaxWidth().padding(vertical = 2.dp),
        color = GlanceTheme.colors.primary,
        backgroundColor = GlanceTheme.colors.secondaryContainer,
    )
}

@Composable
private fun StatusDot(status: WidgetStatus) {
    Spacer(
        GlanceModifier
            .size(10.dp)
            .cornerRadius(5.dp)
            .background(statusColor(status))
    )
}

@Composable
private fun RefreshButton() {
    Text(
        "⟳",
        modifier = GlanceModifier.clickable(actionRunCallback<RefreshAction>()).padding(4.dp),
        style = TextStyle(color = GlanceTheme.colors.primary, fontWeight = FontWeight.Bold),
    )
}

/** An Intent that opens the app at a specific account (deep link). */
private fun accountIntent(context: Context, providerId: String): Intent =
    Intent(Intent.ACTION_VIEW, Uri.parse("quotawidget://account/$providerId")).apply {
        setClassName(context, "tech.allaway.quotawidget.MainActivity")
        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }

/** An Intent that opens the placement configuration for [appWidgetId]. */
private fun configIntent(context: Context, appWidgetId: Int): Intent =
    Intent(context, WidgetConfigActivity::class.java).apply {
        putExtra(android.appwidget.AppWidgetManager.EXTRA_APPWIDGET_ID, appWidgetId)
        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }
