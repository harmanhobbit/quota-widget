package tech.allaway.quotawidget.widget

import java.io.File
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The large-tier period marker draws its bar as a `android.graphics.Bitmap`,
 * and Glance accepts a bitmap only through the **base**
 * `androidx.glance.ImageProvider` factory: the `androidx.glance.appwidget`
 * namesake ships just the Uri overload (glance-appwidget 1.1), so importing
 * that one made `ImageProvider(bitmap)` fail to compile — and only the
 * dispatch-only Android jobs compile this file; the per-push Linux gate never
 * sees Kotlin (issue #191).
 *
 * The JVM suite cannot render Glance, but it can pin the import the compiler
 * resolves the call against. This test reads the widget source as copied into
 * the generated project's app module — the very file that compiles — and
 * asserts the base-package provider is the one in use, so the broken overload
 * cannot sneak back in without a failure naming why.
 */
class GlanceImageProviderSourceTest {
    @Test
    fun markerBitmapUsesTheBasePackageImageProviderFactory() {
        val source = widgetSource().readText()
        assertTrue(
            "QuotaGlanceWidget.kt must import androidx.glance.ImageProvider — the " +
                "only ImageProvider factory that accepts a Bitmap. The " +
                "androidx.glance.appwidget one is Uri-only and cannot compile the " +
                "marker's bitmap call (issue #191).",
            source.contains("import androidx.glance.ImageProvider\n"),
        )
        assertFalse(
            "QuotaGlanceWidget.kt must not import androidx.glance.appwidget.ImageProvider — " +
                "that function group is Uri-only in glance-appwidget 1.1, and importing " +
                "it is what broke the marker's bitmap call (issue #191).",
            source.contains("import androidx.glance.appwidget.ImageProvider\n"),
        )
    }

    /**
     * The patch script copies this source into the generated project's app
     * module (`app/src/main/java/...`), where the unit tests run; the Gradle
     * working directory is the app module itself, but walking up keeps the
     * test independent of where exactly the task was invoked from.
     */
    private fun widgetSource(): File {
        val relative = "src/main/java/tech/allaway/quotawidget/widget/QuotaGlanceWidget.kt"
        var dir: File? = File(System.getProperty("user.dir")).absoluteFile
        while (dir != null) {
            val candidate = File(dir, relative)
            if (candidate.isFile) return candidate
            dir = dir.parentFile
        }
        error(
            "could not locate $relative from ${System.getProperty("user.dir")} — " +
                "the widget host's unit tests run inside the generated Gradle " +
                "project, where patch-android-glance-widget.mjs has copied the " +
                "widget sources into the app module",
        )
    }
}
