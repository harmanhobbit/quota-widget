package tech.allaway.quotawidget.widget

/**
 * The JNI seam into the shared `quota-core` library (issue #113, ADR-0006).
 *
 * A home-screen widget renders with no Tauri activity necessarily alive, but it
 * must not re-decide anything the shared projection already settled — breakpoint
 * selection, per-instance selection, privacy redaction, removed accounts, the
 * shared aggregate colour. So the host calls straight into the same
 * `libquota_widget_lib.so` the app already ships (see `src-tauri/src/
 * widget_jni.rs`): the Rust side loads the persisted stores, projects, flattens
 * and hands back finished JSON the host draws verbatim.
 *
 * Every method is a thin marshal of strings/numbers. The library is loaded once
 * on first use; the same `.so` is already resident when the app process is up,
 * and a cold widget process loads it here.
 */
object WidgetBridge {
    init {
        // The Rust library Tauri builds for the app; the widget shares it.
        System.loadLibrary("quota_widget_lib")
    }

    /**
     * The cold read: project instance [instanceId] at the launcher's current
     * [widthDp] x [heightDp] into the widget-view JSON to draw. [dir] is the
     * app config directory (see [WidgetPaths.configDir]); [nowMillis] ages the
     * read model for the caption. Never fetches, never reads a credential.
     */
    external fun nativeRender(
        dir: String,
        instanceId: String,
        widthDp: Double,
        heightDp: Double,
        nowMillis: Long,
    ): String

    /** The placement activity's account options JSON for [instanceId]. */
    external fun nativeConfigOptions(dir: String, instanceId: String): String

    /**
     * Persist one instance's configuration ([json] matches the widget config
     * shape). Returns an empty string on success, else the error message.
     */
    external fun nativeSaveInstance(dir: String, instanceId: String, json: String): String

    /** Forget an instance the launcher removed. Empty string on success. */
    external fun nativeRemoveInstance(dir: String, instanceId: String): String

    /**
     * The body of the one-time durable refresh work: a real headless refresh
     * that fetches and persists the read model. Empty string on success, else
     * the error. Runs on the WorkManager worker thread, so it outlives the
     * short-lived broadcast receiver that enqueued it.
     *
     * [context] is the worker's `applicationContext`: this process has no Tauri
     * activity, so the Rust side seeds the Keystore-backed secret store from
     * this context instead (see `src-tauri/src/widget_jni.rs`). Without it a
     * background refresh could not decrypt any credential.
     */
    external fun nativeRefresh(dir: String, context: android.content.Context): String
}
