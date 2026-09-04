package tech.allaway.quotawidget.widget

import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.net.Uri
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.graphics.toArgb
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.GlanceTheme
import androidx.glance.Image
// The base package's ImageProvider is the only one with a Bitmap factory —
// androidx.glance.appwidget's namesake carries just the Uri overload in
// glance-appwidget 1.1, so importing that one silently broke the marker's
// bitmap call (issue #191).
import androidx.glance.ImageProvider
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
import androidx.glance.layout.ContentScale
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
import java.time.Instant
import java.time.ZoneId
import java.util.Locale

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

/**
 * The status → colour map. These are deliberately **stable** across light and
 * dark (issue #113: "stable status colours"), so a status reads the same in
 * both themes; only the tier's text/background follow the system theme via
 * [GlanceTheme]. Each is a mid-tone that stays legible on either background,
 * and every status is also carried as a non-colour label ([WidgetStatus.label])
 * so the state never depends on colour alone.
 */
private fun statusColor(status: WidgetStatus): ColorProvider = when (status) {
    WidgetStatus.OK -> ColorProvider(Color(0xFF2E9E4F))
    WidgetStatus.WARN -> ColorProvider(Color(0xFFF9A825))
    WidgetStatus.CRITICAL -> ColorProvider(Color(0xFFE53935))
    WidgetStatus.STALE -> ColorProvider(Color(0xFF9E9E9E))
    WidgetStatus.UNKNOWN -> ColorProvider(Color(0xFF9E9E9E))
}

/** A compact "resets in …" from an epoch-seconds reset instant. */
private fun formatReset(epochSecs: Long): String {
    val delta = epochSecs - System.currentTimeMillis() / 1000
    return when {
        delta <= 0 -> "now"
        delta < 3600 -> "in ${delta / 60}m"
        delta < 86_400 -> "in ${delta / 3600}h"
        else -> "in ${delta / 86_400}d"
    }
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
    // The caption is the absolute local date-time of the read model's last
    // refresh (#195), formatted by the pure helper against the device's
    // timezone and locale. A relative "5m ago" goes stale between widget
    // re-renders — a Glance surface cannot update its own text — so the
    // datetime stays true until the next refresh instead. Absent instant:
    // no caption, never a relative-age fallback.
    content.updatedAtSecs?.let { updated ->
        Text(
            formatUpdatedAt(
                updated,
                ZoneId.systemDefault(),
                Locale.getDefault(),
                Instant.now(),
            ),
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
            val period = cell.period
            if (large && cell.bar != null) {
                if (period != null) {
                    UsageBarWithMarker(cell.bar, period)
                } else {
                    UsageBar(cell.bar)
                }
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

// Geometry of the bitmap-drawn usage bar. The bar is 8dp tall and the tick
// overshoots it by 2dp at each end, like the desktop card's marker.
private const val BAR_HEIGHT_DP = 8f
private const val TICK_WIDTH_DP = 2f
private const val TICK_OVERSHOOT_DP = 2f
private const val BAR_CORNER_DP = 4f

/**
 * The large-tier usage bar with the desktop's period-progress marker (issue
 * #189): the used-percent fill plus a thin tick at `period × width`, so a
 * half-full bar at the quarter mark reads as "burning it fast".
 *
 * Drawn as a single bitmap because Glance offers no fractional layout
 * primitive — Row weights are all-or-nothing (`defaultWeight`, weight 1) and
 * children can't be offset by a fraction of the track — so positioning a tick
 * at an arbitrary point on the bar needs pixels, not view layout. The bitmap
 * is handed to Glance through the base [androidx.glance.ImageProvider]
 * factory (`setImageViewBitmap` under the hood); the `androidx.glance.appwidget`
 * namesake ships only the Uri overload, so the appwidget import is the one
 * thing that must not drift back (issue #191). The bitmap is sized at device
 * density to the bar's real width (the widget's cell width minus the surface's
 * 12dp side padding) and stretched edge-to-edge, so the tick lands at the
 * requested fraction without resampling blur. The marker is a shape (a
 * notch), not a colour change, so it reads without relying on hue.
 */
@Composable
private fun UsageBarWithMarker(barFraction: Double, period: Double) {
    val context = LocalContext.current
    val density = context.resources.displayMetrics.density
    val widthDp = (LocalSize.current.width.value - 24f).coerceIn(120f, 620f)
    val widthPx = (widthDp * density).toInt().coerceAtLeast(1)
    val heightDp = BAR_HEIGHT_DP + 2 * TICK_OVERSHOOT_DP
    val heightPx = (heightDp * density).toInt().coerceAtLeast(1)
    val track = GlanceTheme.colors.secondaryContainer.getColor(context).toArgb()
    val fill = GlanceTheme.colors.primary.getColor(context).toArgb()
    val marker = GlanceTheme.colors.onSurfaceVariant.getColor(context).toArgb()
    val bitmap = remember(barFraction, period, widthPx, heightPx, track, fill, marker) {
        Bitmap.createBitmap(widthPx, heightPx, Bitmap.Config.ARGB_8888).apply {
            val canvas = Canvas(this)
            val paint = Paint(Paint.ANTI_ALIAS_FLAG)
            val barTop = TICK_OVERSHOOT_DP * density
            val barBottom = barTop + BAR_HEIGHT_DP * density
            val radius = BAR_CORNER_DP * density
            // Track, then the used-percent fill (independent of the marker:
            // an overage reading clamps to full while the tick stays put).
            paint.color = track
            canvas.drawRoundRect(
                RectF(0f, barTop, widthPx.toFloat(), barBottom),
                radius, radius, paint,
            )
            paint.color = fill
            val fillRight = (barFraction.coerceIn(0.0, 1.0).toFloat() * widthPx)
                .coerceIn(radius, widthPx.toFloat())
            canvas.drawRoundRect(
                RectF(0f, barTop, fillRight, barBottom),
                radius, radius, paint,
            )
            // The period tick: a thin full-height notch at the fraction.
            paint.color = marker
            val tickWidth = TICK_WIDTH_DP * density
            val tickLeft = period.coerceIn(0.0, 1.0).toFloat() * widthPx - tickWidth / 2f
            canvas.drawRect(
                tickLeft.coerceIn(0f, widthPx - tickWidth),
                0f,
                tickLeft.coerceIn(0f, widthPx - tickWidth) + tickWidth,
                heightPx.toFloat(),
                paint,
            )
        }
    }
    Image(
        provider = ImageProvider(bitmap),
        contentDescription = null,
        modifier = GlanceModifier.fillMaxWidth().height(heightDp.dp),
        contentScale = ContentScale.FillBounds,
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
