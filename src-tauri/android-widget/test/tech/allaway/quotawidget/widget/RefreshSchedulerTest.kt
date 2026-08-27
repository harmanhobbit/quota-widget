package tech.allaway.quotawidget.widget

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The schedule constants are a cross-language contract: `quota_core::refresh::
 * BACKGROUND_REFRESH_TARGET` (Rust) fixes the background refresh target at
 * fifteen minutes, and the app's user-facing copy — and [RefreshScheduler]'s
 * actual periodic request — must describe the same number (issue #111). This
 * file is the Kotlin side of that agreement; the constant cannot be read from
 * Rust here, so the test pins it and points at both sources of truth, making
 * any change to the target a deliberate, three-place edit.
 */
class RefreshSchedulerTest {
    @Test
    fun periodicTargetIsTheDocumentedFifteenMinuteBackgroundRefreshTarget() {
        assertEquals(15L, RefreshScheduler.PERIODIC_MINUTES)
    }
}
