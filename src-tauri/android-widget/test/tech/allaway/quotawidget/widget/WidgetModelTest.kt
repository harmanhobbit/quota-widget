package tech.allaway.quotawidget.widget

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The Kotlin-side unit tests issue #113 asks for: they exercise the parse of
 * the `WidgetView` wire format the shared library produces (`quota_core::
 * widget_view`), which is the whole of the host's own decision surface — every
 * projection, breakpoint, privacy and removed-account decision was already made
 * and tested in Rust. Because the JSON here is hand-written to match that wire
 * format, these tests also fail loudly if the two ever drift.
 */
class WidgetModelTest {
    @Test
    fun parsesLargeContentWithRowsBarsAndReset() {
        val json = """
            {
              "size": "large",
              "state": "content",
              "content": {
                "aggregate_status": "warn",
                "aggregate_pct": 42.0,
                "data_age_secs": 90,
                "updated_at_secs": 1750000000,
                "privacy": false,
                "rows": [
                  {
                    "provider_id": "a",
                    "name": "Acme",
                    "removed": false,
                    "status": "ok",
                    "cells": [
                      { "label": "Window", "value": "42%", "bar": 0.42, "resets_at_secs": 100, "period": 0.25 }
                    ]
                  }
                ]
              }
            }
        """.trimIndent()

        val view = parseWidgetView(json)
        assertEquals("large", view.size)
        assertEquals(WidgetState.CONTENT, view.state)
        val content = view.content!!
        assertEquals(WidgetStatus.WARN, content.aggregateStatus)
        assertEquals(90L, content.dataAgeSecs)
        assertEquals(1750000000L, content.updatedAtSecs)
        assertFalse(content.privacy)
        val row = content.rows.single()
        assertFalse(row.removed)
        assertEquals(WidgetStatus.OK, row.status)
        val cell = row.cells.single()
        assertEquals("Window", cell.label)
        assertEquals("42%", cell.value)
        assertEquals(0.42, cell.bar!!, 1e-9)
        assertEquals(100L, cell.resetsAtSecs)
        assertEquals(0.25, cell.period!!, 1e-9)
    }

    @Test
    fun parsesTheThreePlaceholderStates() {
        assertEquals(
            WidgetState.NEEDS_CONFIGURATION,
            parseWidgetView("""{"size":"large","state":"needs_configuration"}""").state,
        )
        assertEquals(
            WidgetState.NO_DATA,
            parseWidgetView("""{"size":"medium","state":"no_data"}""").state,
        )
        val unknown = parseWidgetView("""{"size":"small","state":"nonsense"}""")
        assertEquals(WidgetState.UNKNOWN, unknown.state)
        assertNull(unknown.content)
    }

    @Test
    fun aRemovedRowHasNoStatusAndNoCells() {
        val json = """
            {
              "size": "medium",
              "state": "content",
              "content": {
                "aggregate_status": "ok",
                "aggregate_pct": 0.0,
                "privacy": false,
                "rows": [
                  { "provider_id": "gone", "name": "gone", "removed": true, "cells": [] }
                ]
              }
            }
        """.trimIndent()
        val row = parseWidgetView(json).content!!.rows.single()
        assertTrue(row.removed)
        assertNull("a removed row carries no status", row.status)
        assertTrue(row.cells.isEmpty())
    }

    @Test
    fun aMutedRowOmittedFromTheWireParsesToExactlyTheSurvivingRows() {
        // The projection drops a muted account (a deliberately-empty headline
        // selection) before the wire (#197), so the host never sees it and does
        // not need to know it existed: what arrives is exactly what draws
        // (ADR-0006 — the host only renders).
        val json = """
            {
              "size": "medium",
              "state": "content",
              "content": {
                "aggregate_status": "ok",
                "aggregate_pct": 30.0,
                "privacy": false,
                "rows": [
                  {
                    "provider_id": "live", "name": "Live", "removed": false, "status": "ok",
                    "cells": [ { "label": "Live", "value": "30%" } ]
                  }
                ]
              }
            }
        """.trimIndent()
        val content = parseWidgetView(json).content!!
        assertEquals("the muted row never arrives", 1, content.rows.size)
        val row = content.rows.single()
        assertEquals("live", row.providerId)
        assertFalse(row.removed)
        assertEquals(WidgetStatus.OK, row.status)
        assertEquals("30%", row.cells.single().value)
    }

    @Test
    fun aPresentRowWithEmptyCellsStillParsesAsPresent() {
        // A non-removed row with empty cells (a stale/failed account whose
        // automatic pick found nothing) still parses present with its name,
        // status and empty cells: the host renders what arrives and performs
        // no independent drop — filtering here would hide stale accounts
        // (ADR-0006), which is exactly what the #197 fix avoids.
        val json = """
            {
              "size": "medium",
              "state": "content",
              "content": {
                "aggregate_status": "stale",
                "aggregate_pct": 0.0,
                "privacy": false,
                "rows": [
                  { "provider_id": "stale", "name": "Stale", "removed": false, "status": "stale", "cells": [] }
                ]
              }
            }
        """.trimIndent()
        val row = parseWidgetView(json).content!!.rows.single()
        assertFalse(row.removed)
        assertEquals("Stale", row.name)
        assertEquals(WidgetStatus.STALE, row.status)
        assertTrue(row.cells.isEmpty())
    }

    @Test
    fun aRedactedCellHasNoValueButKeepsLabelAndReset() {
        val json = """
            {
              "size": "large",
              "state": "content",
              "content": {
                "aggregate_status": "ok",
                "aggregate_pct": 0.0,
                "privacy": true,
                "rows": [
                  {
                    "provider_id": "a", "name": "A", "removed": false, "status": "ok",
                    "cells": [ { "label": "Rolling", "resets_at_secs": 200 } ]
                  }
                ]
              }
            }
        """.trimIndent()
        val content = parseWidgetView(json).content!!
        assertTrue(content.privacy)
        val cell = content.rows.single().cells.single()
        assertEquals("Rolling", cell.label)
        assertNull("a redacted figure is absent", cell.value)
        assertNull("no bar for a redacted cell", cell.bar)
        assertNull("the marker rides on the bar, so it is redacted too", cell.period)
        assertEquals(200L, cell.resetsAtSecs)
    }

    @Test
    fun anUnboundedWindowParsesNoPeriodMarker() {
        // The wire format omits `period` when the provider reports no period
        // bounds (most windows) — the parse must tolerate the absence.
        val json = """
            {
              "size": "large",
              "state": "content",
              "content": {
                "aggregate_status": "ok",
                "aggregate_pct": 0.0,
                "privacy": false,
                "rows": [
                  {
                    "provider_id": "a", "name": "A", "removed": false, "status": "ok",
                    "cells": [ { "label": "Rolling", "value": "42%", "bar": 0.42 } ]
                  }
                ]
              }
            }
        """.trimIndent()
        val cell = parseWidgetView(json).content!!.rows.single().cells.single()
        assertEquals(0.42, cell.bar!!, 1e-9)
        assertNull("no bounds, no marker", cell.period)
    }

    @Test
    fun parsesTheSmallTierWorstHeadline() {
        val json = """
            {
              "size": "small",
              "state": "content",
              "content": {
                "aggregate_status": "critical",
                "aggregate_pct": 95.0,
                "privacy": false,
                "worst": {
                  "name": "Busy",
                  "status": "critical",
                  "cell": { "label": "Busy", "value": "95%" }
                },
                "rows": []
              }
            }
        """.trimIndent()
        val content = parseWidgetView(json).content!!
        assertTrue("small tier lays out no rows", content.rows.isEmpty())
        val worst = content.worst!!
        assertEquals("Busy", worst.name)
        assertEquals(WidgetStatus.CRITICAL, worst.status)
        assertEquals("95%", worst.cell.value)
    }

    @Test
    fun statusMappingCoversEveryVariant() {
        assertEquals(WidgetStatus.OK, WidgetStatus.from("ok"))
        assertEquals(WidgetStatus.WARN, WidgetStatus.from("warn"))
        assertEquals(WidgetStatus.CRITICAL, WidgetStatus.from("critical"))
        assertEquals(WidgetStatus.STALE, WidgetStatus.from("stale"))
        assertEquals(WidgetStatus.UNKNOWN, WidgetStatus.from(null))
        assertEquals(WidgetStatus.UNKNOWN, WidgetStatus.from("something-else"))
    }

    @Test
    fun missingOptionalFieldsDoNotThrow() {
        // A minimal content object: no worst, no data_age, no updated_at, no
        // rows array.
        val json = """
            {
              "size": "medium",
              "state": "content",
              "content": { "aggregate_status": "ok", "aggregate_pct": 0.0, "privacy": false }
            }
        """.trimIndent()
        val content = parseWidgetView(json).content!!
        assertNull(content.dataAgeSecs)
        assertNull("an absent last-update instant parses to null (#195)", content.updatedAtSecs)
        assertNull(content.worst)
        assertTrue(content.rows.isEmpty())
    }
}
