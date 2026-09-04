package tech.allaway.quotawidget.widget

import org.json.JSONObject

/**
 * The parsed shape of the `WidgetView` JSON the Rust side hands back
 * (`quota_core::widget_view`). This is a verbatim mirror of the wire types — no
 * decisions, just the parse — so the rendering code (and its unit tests) work
 * against typed Kotlin rather than raw JSON. Every quota decision was already
 * made in `quota-core`; the host only draws these values.
 */

/** One of the three placeholder states, or real content. */
enum class WidgetState { NEEDS_CONFIGURATION, NO_DATA, CONTENT, UNKNOWN }

/**
 * The shared status a cell/row/aggregate carries. Mapped to a colour by the
 * renderer, but also to a non-colour cue ([label]) so the state is legible
 * without relying on colour alone (issue #113: "non-colour state cues").
 */
enum class WidgetStatus(val label: String) {
    OK("OK"),
    WARN("Warning"),
    CRITICAL("Critical"),
    STALE("Stale"),
    UNKNOWN("Unknown");

    companion object {
        fun from(s: String?): WidgetStatus = when (s) {
            "ok" -> OK
            "warn" -> WARN
            "critical" -> CRITICAL
            "stale" -> STALE
            else -> UNKNOWN
        }
    }
}

data class WidgetView(
    val size: String,
    val state: WidgetState,
    val content: WidgetContent?,
)

data class WidgetContent(
    val aggregateStatus: WidgetStatus,
    val aggregatePct: Double,
    val dataAgeSecs: Long?,
    /**
     * The instant [dataAgeSecs] is measured from, as epoch seconds (#195) —
     * the read model's own refresh stamp, for the absolute "Updated …"
     * caption. Null when absent; the caption is then omitted entirely.
     */
    val updatedAtSecs: Long?,
    val privacy: Boolean,
    val worst: WorstHeadline?,
    val rows: List<WidgetRow>,
)

data class WorstHeadline(
    val name: String,
    val status: WidgetStatus,
    val cell: HeadlineCell,
)

data class WidgetRow(
    val providerId: String,
    val name: String,
    val removed: Boolean,
    val status: WidgetStatus?,
    val cells: List<HeadlineCell>,
)

data class HeadlineCell(
    val label: String,
    /** The pre-formatted figure, or null when privacy-redacted or absent. */
    val value: String?,
    /** Usage bar fraction in 0.0..1.0, large tier only. */
    val bar: Double?,
    /** Reset instant as epoch seconds, large tier only. */
    val resetsAtSecs: Long?,
    /**
     * Period-progress fraction in 0.0..1.0, large tier only — where the
     * desktop's period marker sits on the usage bar. Computed in quota-core
     * (`period.js` semantics); the host draws it verbatim at `period × width`.
     * Null when the provider reports no period bounds, or the bar itself is
     * absent (below the large tier, or privacy-redacted).
     */
    val period: Double?,
)

/** Parse the widget-view JSON. Never throws on a missing optional field. */
fun parseWidgetView(json: String): WidgetView {
    val root = JSONObject(json)
    val state = when (root.optString("state")) {
        "needs_configuration" -> WidgetState.NEEDS_CONFIGURATION
        "no_data" -> WidgetState.NO_DATA
        "content" -> WidgetState.CONTENT
        else -> WidgetState.UNKNOWN
    }
    val content = root.optJSONObject("content")?.let { parseContent(it) }
    return WidgetView(size = root.optString("size", "small"), state = state, content = content)
}

private fun parseContent(obj: JSONObject): WidgetContent {
    val rows = obj.optJSONArray("rows")?.let { arr ->
        (0 until arr.length()).map { parseRow(arr.getJSONObject(it)) }
    } ?: emptyList()
    val worst = obj.optJSONObject("worst")?.let {
        WorstHeadline(
            name = it.optString("name"),
            status = WidgetStatus.from(it.optString("status")),
            cell = parseCell(it.getJSONObject("cell")),
        )
    }
    return WidgetContent(
        aggregateStatus = WidgetStatus.from(obj.optString("aggregate_status")),
        aggregatePct = obj.optDouble("aggregate_pct", 0.0),
        dataAgeSecs = obj.optLongOrNull("data_age_secs"),
        updatedAtSecs = obj.optLongOrNull("updated_at_secs"),
        privacy = obj.optBoolean("privacy", false),
        worst = worst,
        rows = rows,
    )
}

private fun parseRow(obj: JSONObject): WidgetRow {
    val cells = obj.optJSONArray("cells")?.let { arr ->
        (0 until arr.length()).map { parseCell(arr.getJSONObject(it)) }
    } ?: emptyList()
    return WidgetRow(
        providerId = obj.optString("provider_id"),
        name = obj.optString("name"),
        removed = obj.optBoolean("removed", false),
        status = if (obj.has("status")) WidgetStatus.from(obj.optString("status")) else null,
        cells = cells,
    )
}

private fun parseCell(obj: JSONObject): HeadlineCell = HeadlineCell(
    label = obj.optString("label"),
    value = if (obj.has("value")) obj.optString("value") else null,
    bar = if (obj.has("bar")) obj.optDouble("bar") else null,
    resetsAtSecs = obj.optLongOrNull("resets_at_secs"),
    period = if (obj.has("period")) obj.optDouble("period") else null,
)

/** `optLong` treats a missing key as 0; this keeps a genuinely-absent field null. */
private fun JSONObject.optLongOrNull(key: String): Long? =
    if (has(key) && !isNull(key)) getLong(key) else null
