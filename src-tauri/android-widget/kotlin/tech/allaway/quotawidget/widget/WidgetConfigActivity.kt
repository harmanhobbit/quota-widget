package tech.allaway.quotawidget.widget

import android.app.Activity
import android.appwidget.AppWidgetManager
import android.content.Intent
import android.os.Bundle
import android.view.Gravity
import android.view.ViewGroup
import android.widget.Button
import android.widget.CheckBox
import android.widget.LinearLayout
import android.widget.Switch
import android.widget.TextView
import org.json.JSONArray
import org.json.JSONObject

/**
 * The placement configuration shown when a widget is dropped, and reopened via
 * an "unconfigured" / removed-account tap (issue #113: "placing a widget opens
 * configuration; multiple instances retain independent selections").
 *
 * It is a plain View-based screen (no Compose) so it stays small: a checkbox per
 * enabled account, a privacy switch, and OK. The options — including which
 * accounts a fresh placement inherits from the shared compact summary — come
 * from the shared library ([WidgetBridge.nativeConfigOptions]); the chosen
 * selection is persisted through it ([WidgetBridge.nativeSaveInstance]), so this
 * activity makes no quota decision of its own.
 */
class WidgetConfigActivity : Activity() {
    private var appWidgetId = AppWidgetManager.INVALID_APPWIDGET_ID
    private val checkBoxes = mutableListOf<Pair<String, CheckBox>>()
    private lateinit var privacySwitch: Switch

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Until OK is pressed, a placement is treated as cancelled: if the user
        // backs out, the launcher must not add a half-configured widget.
        setResult(RESULT_CANCELED)

        appWidgetId = intent?.extras?.getInt(
            AppWidgetManager.EXTRA_APPWIDGET_ID,
            AppWidgetManager.INVALID_APPWIDGET_ID,
        ) ?: AppWidgetManager.INVALID_APPWIDGET_ID
        if (appWidgetId == AppWidgetManager.INVALID_APPWIDGET_ID) {
            finish()
            return
        }

        val dir = WidgetPaths.configDir(this)
        val options = JSONObject(WidgetBridge.nativeConfigOptions(dir, appWidgetId.toString()))

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(32, 32, 32, 32)
        }
        root.addView(TextView(this).apply {
            text = "Choose accounts for this widget"
            textSize = 18f
        })

        val accounts = options.optJSONArray("accounts") ?: JSONArray()
        for (i in 0 until accounts.length()) {
            val account = accounts.getJSONObject(i)
            val id = account.optString("provider_id")
            val box = CheckBox(this).apply {
                text = account.optString("name", id)
                isChecked = account.optBoolean("selected", false)
            }
            checkBoxes.add(id to box)
            root.addView(box)
        }

        privacySwitch = Switch(this).apply {
            text = "Privacy mode (hide figures)"
            isChecked = options.optBoolean("privacy", false)
        }
        root.addView(privacySwitch)

        root.addView(Button(this).apply {
            text = "OK"
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.WRAP_CONTENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ).apply { gravity = Gravity.END }
            setOnClickListener { save(dir) }
        })

        setContentView(root)
    }

    private fun save(dir: String) {
        val selected = JSONArray()
        for ((id, box) in checkBoxes) {
            if (box.isChecked) {
                // headlines omitted → inherit the shared selection for this
                // account (the widget config's per-account `None`).
                selected.put(JSONObject().put("provider_id", id))
            }
        }
        val config = JSONObject()
            .put("accounts", selected)
            .put("privacy", privacySwitch.isChecked)

        val error = WidgetBridge.nativeSaveInstance(dir, appWidgetId.toString(), config.toString())
        if (error.isNotEmpty()) {
            TextView(this).apply { text = error }
            // Leave the result cancelled on a save failure so the launcher does
            // not place a widget with no persisted preference.
            return
        }

        // Enqueue a refresh so the freshly-placed widget renders from real data
        // (the worker re-renders every instance when it finishes) rather than
        // the system's initial layout, then report success with the widget id.
        WidgetRefreshWorker.enqueue(applicationContext)
        setResult(RESULT_OK, Intent().putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, appWidgetId))
        finish()
    }
}
