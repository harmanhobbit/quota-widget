package tech.allaway.quotawidget.widget

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import org.json.JSONArray
import tech.allaway.quotawidget.R

/**
 * One planned notification, parsed from the wire JSON
 * `quota_core::alerts::plan_notifications` produces. This is a verbatim mirror
 * of the Rust DTO — no decisions, just the parse — exactly like `WidgetModel`
 * for the widget view. Every decision was already made in `quota-core`: the
 * per-account toast toggles and the baseline rule filtered this list, and the
 * full/public text pair below is the redaction policy itself.
 */
data class NotificationPlanEntry(
    val providerId: String,
    val level: String,
    val title: String,
    val body: String,
    val publicTitle: String,
    val publicBody: String,
)

/**
 * Parse the plan JSON (a Rust `Vec<PlannedNotification>`, serialized with
 * serde's default snake_case field names). Missing fields degrade to empty
 * strings rather than throwing, matching `WidgetModel`'s parse tolerance; an
 * empty plan posts nothing.
 */
fun parseNotificationPlan(json: String): List<NotificationPlanEntry> {
    val root = JSONArray(json)
    return (0 until root.length()).map { i ->
        val obj = root.getJSONObject(i)
        val content = obj.optJSONObject("content")
        NotificationPlanEntry(
            providerId = obj.optString("provider_id"),
            level = obj.optString("level"),
            title = content?.optString("title") ?: "",
            body = content?.optString("body") ?: "",
            publicTitle = content?.optString("public_title") ?: "",
            publicBody = content?.optString("public_body") ?: "",
        )
    }
}

/**
 * The native host's notification poster (issue #112, ADR-0006). Rust decides
 * *whether* anything should be posted and *what it says* (both the full text
 * and the generic lock-screen text); this class only turns that plan into real
 * system notifications.
 *
 * Privacy is a platform property, not a rendering choice: every notification
 * is posted at `VISIBILITY_PRIVATE` with the plan's generic `public_title` /
 * `public_body` as its public version, so a locked device shows "Quota alert —
 * open Quota Widget to view details" and nothing else, while the unlocked
 * notification names the account and the figure. Redacting here would trust
 * the host with the redaction rule; the rule lives in `quota-core` and the
 * JVM tests on the Rust side pin it.
 *
 * Reached over JNI from both refresh hosts — the foreground app
 * (`mobile.rs::refresh_once`, via `android_notifications::deliver`) and the
 * WorkManager worker (`widget_jni.rs::headless_refresh`, via
 * `deliver_with_env`) — so a crossing notifies once no matter which host ran
 * the pass. Called with the application context; never assumes an Activity.
 */
object AlertNotifier {
    private const val TAG = "QuotaAlerts"

    /** One channel for all quota alerts; importance is fixed at creation. */
    private const val CHANNEL_ID = "quota-alerts"

    /**
     * Post every entry in [planJson] (the marshalled
     * `quota_core::alerts::plan_notifications_json`). Best-effort: a missing
     * permission or a failed post is logged, never thrown — the refresh that
     * produced the plan has already persisted its read model by the time this
     * runs, and a notification must never take a refresh down.
     */
    @JvmStatic
    fun deliver(context: Context, planJson: String) {
        val plan = parseNotificationPlan(planJson)
        if (plan.isEmpty()) return

        val manager = NotificationManagerCompat.from(context)
        // Without POST_NOTIFICATIONS (denied after the one-time ask, or not
        // yet granted) notify() silently drops or throws — check once here so
        // the skip is observable in logcat rather than mysterious.
        if (!manager.areNotificationsEnabled()) {
            Log.i(TAG, "notifications disabled by permission/settings; plan of ${plan.size} dropped")
            return
        }
        ensureChannel(manager)

        // Tapping the notification opens the app itself: whatever launch
        // intent the launcher uses, so no Activity class is hardcoded here.
        val launch = context.packageManager.getLaunchIntentForPackage(context.packageName)
        val contentIntent = PendingIntent.getActivity(
            context,
            0,
            launch ?: Intent(),
            PendingIntent.FLAG_IMMUTABLE,
        )

        for (entry in plan) {
            // The lock-screen form: deliberately generic, straight from the
            // plan, so no detail leaks through the public version.
            val publicVersion = builder(context, entry.publicTitle, entry.publicBody, contentIntent)
                .setVisibility(Notification.VISIBILITY_PUBLIC)
                .build()
            // The unlocked form. VISIBILITY_PRIVATE is what makes Android
            // substitute the public version on a locked device.
            val notification = builder(context, entry.title, entry.body, contentIntent)
                .setStyle(NotificationCompat.BigTextStyle().bigText(entry.body))
                .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
                .setPublicVersion(publicVersion)
                .setCategory(NotificationCompat.CATEGORY_STATUS)
                // Stable per account: a re-notify for the same account
                // replaces rather than stacks; different accounts coexist.
                .build()
            try {
                manager.notify(entry.providerId.hashCode(), notification)
            } catch (e: SecurityException) {
                // The permission was revoked between the enabled check and
                // here (or by an OEM policy). Nothing to do but stop cleanly.
                Log.w(TAG, "notification not posted (security): ${e.message}")
                return
            }
        }
    }

    /** Shared builder: small icon, channel, tap action, auto-dismiss on tap. */
    private fun builder(
        context: Context,
        title: String,
        text: String,
        contentIntent: PendingIntent,
    ): NotificationCompat.Builder =
        NotificationCompat.Builder(context, CHANNEL_ID)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(contentIntent)
            .setAutoCancel(true)

    /** Idempotent: creating an existing channel with the same values is a no-op. */
    private fun ensureChannel(manager: NotificationManagerCompat) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val channel = NotificationChannel(
            CHANNEL_ID,
            "Quota alerts",
            // DEFAULT, not HIGH: threshold warnings are worth a drawer entry,
            // not a heads-up interruption; the channel owns this on O+.
            NotificationManager.IMPORTANCE_DEFAULT,
        ).apply { description = "Fires when an account crosses a warn or critical threshold" }
        manager.createNotificationChannel(channel)
    }
}
