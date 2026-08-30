package tech.allaway.quotawidget.widget

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.util.Log
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat

/**
 * The permission half of the notification seam (issue #112), kept separate
 * from [AlertNotifier]'s posting so the worker-only paths never touch Activity
 * APIs. Reached over JNI from `android_notifications.rs`:
 *
 * - [state] feeds the mobile Settings row (the system's own truth, read live).
 * - [request] fires the single POST_NOTIFICATIONS dialog from the foreground
 *   Activity — invoked only when the Rust one-shot decision has said so (first
 *   successful account, notifications wanted, never asked before, flag
 *   persisted in platform preferences). It reports whether the dialog actually
 *   started: `false` means no Activity is reachable from the given context, and
 *   Rust then leaves the one-shot unburned. The grant/denial result itself is
 *   not plumbed back: the OS records it, [state] reads it live, and the
 *   Rust-side flag records that the *request was issued*, which is the thing
 *   that must never happen twice.
 * - [openSettings] is the no-re-prompt recovery path once that one-shot is
 *   spent: the system's notification settings for this app.
 */
object NotificationAccess {
    private const val TAG = "QuotaAlerts"

    /**
     * Arbitrary but stable request code. The result lands in the Activity's
     * `onRequestPermissionsResult`, which nothing here hooks: there is no
     * in-app UI that must react instantly, and the next [state] read (and the
     * system itself) already knows the answer.
     */
    private const val REQUEST_CODE = 4712

    /**
     * `"granted"` or `"denied"`. Below Android 13 the runtime permission does
     * not exist and notifications are governed only by the system-settings
     * toggle, so the permission question is trivially `"granted"`.
     */
    @JvmStatic
    fun state(context: Context): String {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return "granted"
        val granted = ContextCompat.checkSelfPermission(
            context,
            Manifest.permission.POST_NOTIFICATIONS,
        ) == PackageManager.PERMISSION_GRANTED
        return if (granted) "granted" else "denied"
    }

    /**
     * Show the system permission dialog, reporting whether the request was
     * issued. No-op below Android 13, where there is nothing to request
     * ([state] is already `"granted"` there) — reported as issued, since the
     * ask is trivially complete.
     *
     * The Activity is found by walking the [ContextWrapper] chain, never by
     * casting: JNI verifies no argument types, so a blind cast on the Rust
     * side would hand a non-Activity to [ActivityCompat.requestPermissions]
     * and die in the callee's type check instead of failing the call. When
     * nothing in the chain is an Activity, `false` lets Rust leave the
     * one-shot unspent for a later foreground pass.
     */
    @JvmStatic
    fun request(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return true
        var probe: Context = context
        while (probe is ContextWrapper) {
            if (probe is Activity) {
                ActivityCompat.requestPermissions(
                    probe,
                    arrayOf(Manifest.permission.POST_NOTIFICATIONS),
                    REQUEST_CODE,
                )
                return true
            }
            probe = probe.baseContext
        }
        return false
    }

    /**
     * Open the system's notification settings for this app. On API levels
     * without the specific notification screen (below 26) — or if an OEM
     * removes it — fall back to the app's own details page, which contains the
     * same toggle one level down. Best-effort: a launcher that resolves
     * neither just logs.
     */
    @JvmStatic
    fun openSettings(context: Context) {
        val intent = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS)
                .putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
        } else {
            Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS)
                .setData(Uri.fromParts("package", context.packageName, null))
        }
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        try {
            context.startActivity(intent)
        } catch (e: ActivityNotFoundException) {
            Log.w(TAG, "no notification settings screen resolved: ${e.message}")
        }
    }
}
