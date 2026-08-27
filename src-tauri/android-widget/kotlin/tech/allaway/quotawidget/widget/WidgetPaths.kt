package tech.allaway.quotawidget.widget

import android.content.Context
import android.os.Build

/**
 * Where the persisted read model, config and widget preferences live.
 *
 * The foreground host writes these through Tauri's `app_config_dir()`
 * (`src-tauri/src/mobile.rs`). On Android that call is **not** `filesDir`: Tauri
 * routes `app_config_dir()` to its `getConfigDir` path command, which returns
 * `activity.dataDir` — the data-directory *root* (`/data/data/<pkg>`), API 24+,
 * falling back to `applicationInfo.dataDir` below N. `filesDir` is that root's
 * `/files` subdirectory, a *different* directory: reading it finds no
 * `shared-config.json`, so `Config::load` falls back to the desktop default
 * (which pre-enables Claude and Codex) and the refresh worker then writes stale
 * snapshots into the wrong place. So this must mirror `getConfigDir` exactly —
 * `dataDir`, not `filesDir` — for the widget to read what the app wrote. A wrong
 * path only ever degrades to the honest placeholder, never a crash; nothing here
 * leaves the app sandbox (ADR-0006).
 */
object WidgetPaths {
    fun configDir(context: Context): String =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            context.dataDir.absolutePath
        } else {
            context.applicationInfo.dataDir
        }
}
