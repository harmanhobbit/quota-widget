package tech.allaway.quotawidget.widget

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Drift guard for the notification plan's wire format (issue #112): the JSON
 * is produced by `quota_core::alerts::plan_notifications_json` and marshalled
 * verbatim across JNI, so a Rust-side field rename would silently break
 * posting. These tests pin the exact snake_case keys the parse reads, the same
 * way `WidgetModelTest` pins the widget-view wire.
 */
class NotificationPlanTest {
    /** The literal shape serde emits for `Vec<PlannedNotification>`. */
    private val planJson = """
        [
          {
            "provider_id": "claude",
            "level": "critical",
            "content": {
              "title": "Claude — critical",
              "body": "5-hour window at 96%",
              "public_title": "Quota alert",
              "public_body": "Open Quota Widget to view details"
            }
          },
          {
            "provider_id": "openrouter",
            "level": "warn",
            "content": {
              "title": "OpenRouter — warning",
              "body": "balance low: 1.42 USD",
              "public_title": "Quota alert",
              "public_body": "Open Quota Widget to view details"
            }
          }
        ]
    """.trimIndent()

    @Test
    fun parses_the_wire_shape_quota_core_serializes() {
        val plan = parseNotificationPlan(planJson)
        assertEquals(2, plan.size)

        val first = plan[0]
        assertEquals("claude", first.providerId)
        assertEquals("critical", first.level)
        assertEquals("Claude — critical", first.title)
        assertEquals("5-hour window at 96%", first.body)
        assertEquals("Quota alert", first.publicTitle)
        assertEquals("Open Quota Widget to view details", first.publicBody)

        assertEquals("openrouter", plan[1].providerId)
        assertEquals("warn", plan[1].level)
    }

    @Test
    fun an_empty_plan_posts_nothing() {
        assertTrue(parseNotificationPlan("[]").isEmpty())
    }

    @Test
    fun missing_fields_degrade_to_empty_strings_without_throwing() {
        // A plan whose content object is absent entirely: the parse must not
        // throw (the marshal is mechanical and trusted, but a version skew
        // degrades to "post nothing readable", not a crash in the worker).
        val sparse = """[{"provider_id": "claude", "level": "warn"}]"""
        val plan = parseNotificationPlan(sparse)
        assertEquals(1, plan.size)
        assertEquals("claude", plan[0].providerId)
        assertEquals("", plan[0].title)
        assertEquals("", plan[0].publicBody)
    }
}
