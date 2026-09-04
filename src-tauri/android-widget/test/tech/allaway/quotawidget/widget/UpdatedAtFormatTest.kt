package tech.allaway.quotawidget.widget

import java.time.Instant
import java.time.ZoneId
import java.util.Locale
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pins the widget's "Updated …" caption (#195): the host formats the read
 * model's absolute last-update instant against the device's timezone and
 * locale — the same host-side absolute-time treatment `resets_at_secs` already
 * gets, because wall-clock presentation belongs where the device clock is
 * (ADR-0006). The helper is pure, so fixed zone/locale/now inputs make the
 * assertions exact.
 *
 * [Locale.UK] renders 24-hour times ("15:32"), avoiding the narrow
 * no-break space newer CLDR data puts before AM/PM, and
 * [ZoneId.of]("+01:00") is a fixed offset with no DST history: both keep the
 * expected strings stable across JVMs. The +01:00 offset is also the UTC
 * guard — an instant whose UTC wall time reads differently catches a
 * formatter that ignored the supplied zone.
 */
class UpdatedAtFormatTest {
    private val zone = ZoneId.of("+01:00")
    private val locale = Locale.UK

    // 2026-01-15T14:32:00Z — 15:32 on Jan 15 in the +01:00 zone.
    private val updated = Instant.parse("2026-01-15T14:32:00Z").epochSecond

    @Test
    fun aSameDayInstantRendersTheLocalizedTimeOnly() {
        val now = Instant.parse("2026-01-15T17:45:00Z")
        assertEquals("Updated 15:32", formatUpdatedAt(updated, zone, locale, now))
    }

    @Test
    fun anEarlierDayInstantRendersTheLocalizedShortDateAndTime() {
        // `now` is Jan 16 local (09:00Z = 10:00 in +01:00), so Jan 15 is the
        // earlier day and its *local* date — 15/01, not the UTC date — is
        // rendered with the time.
        val now = Instant.parse("2026-01-16T09:00:00Z")
        assertEquals("Updated 15/01/2026, 15:32", formatUpdatedAt(updated, zone, locale, now))
    }

    @Test
    fun theInstantRendersInTheSuppliedZoneNotUtc() {
        val now = Instant.parse("2026-01-15T17:45:00Z")
        val text = formatUpdatedAt(updated, zone, locale, now)
        assertTrue("expected the +01:00 wall time in: $text", text.contains("15:32"))
        assertFalse(
            "a UTC render would show 14:32 — the supplied zone was ignored: $text",
            text.contains("14:32"),
        )
    }

    @Test
    fun theCaptionIsPrefixedUpdated() {
        val now = Instant.parse("2026-01-15T17:45:00Z")
        assertTrue(formatUpdatedAt(updated, zone, locale, now).startsWith("Updated "))
    }
}
