package tech.allaway.quotawidget.widget

import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale

/**
 * Host-side presentation of the read model's last-update instant (issue #195).
 *
 * `quota-core` carries the authoritative instant across the wire as
 * `updated_at_secs` ([WidgetContent.updatedAtSecs]) — the same instant its
 * relative `data_age_secs` is derived from. Formatting it as an absolute
 * local datetime happens here, in the host, exactly like `resets_at_secs`:
 * wall-clock and locale presentation belong where the device clock is, which
 * is consistent with ADR-0006 (the host still makes no quota decision — it
 * only draws).
 *
 * A relative age ("5m ago") goes stale between widget re-renders because a
 * Glance surface cannot update its own text; an absolute datetime stays true
 * until the next refresh.
 *
 * The helper is pure — zone, locale and `now` all come from the caller (the
 * composable passes the device's `ZoneId.systemDefault()` /
 * `Locale.getDefault()` / `Instant.now()`) — so the JVM unit tests pin the
 * formatting exactly, with no Android dependency in sight.
 *
 * Same calendar day as `now`: time only ("Updated 15:32"); an earlier day:
 * localized short date plus time ("Updated 14/01/2026, 15:32").
 */
internal fun formatUpdatedAt(
    epochSecs: Long,
    zone: ZoneId,
    locale: Locale,
    now: Instant,
): String {
    val updated = Instant.ofEpochSecond(epochSecs).atZone(zone)
    val time = DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT)
        .withLocale(locale)
        .format(updated)
    return if (updated.toLocalDate() == now.atZone(zone).toLocalDate()) {
        "Updated $time"
    } else {
        val date = DateTimeFormatter.ofLocalizedDate(FormatStyle.SHORT)
            .withLocale(locale)
            .format(updated)
        "Updated $date, $time"
    }
}
