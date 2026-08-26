package tech.allaway.quotawidget.widget

import android.content.Context

/**
 * Where the persisted read model, config and widget preferences live.
 *
 * The foreground host writes these through Tauri's `app_config_dir()`
 * (`src-tauri/src/mobile.rs`), which on Android resolves to the app's private
 * files directory. The widget process is the same app, so it reads the same
 * `context.filesDir` — the one directory both the app and every widget instance
 * agree on, with no world-readable data leaving the sandbox (ADR-0006). A wrong
 * path degrades to the honest "No data—tap to refresh", never a crash.
 */
object WidgetPaths {
    fun configDir(context: Context): String = context.filesDir.absolutePath
}
