package tech.allaway.quotawidget.widget

import androidx.work.NetworkType
import java.util.concurrent.TimeUnit
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The schedule constants and the request they build are a cross-language
 * contract: `quota_core::refresh::BACKGROUND_REFRESH_TARGET` (Rust) fixes the
 * background refresh target at fifteen minutes, and the app's user-facing copy
 * — and [RefreshScheduler]'s actual periodic request — must describe the same
 * number (issue #111). This file is the Kotlin side of that agreement; the
 * constant cannot be read from Rust here, so the tests pin it and point at both
 * sources of truth, making any change to the target a deliberate, three-place
 * edit.
 *
 * The configuration test goes further than the constant: it asserts the request
 * [RefreshScheduler.ensurePeriodic] actually enqueues — interval, worker class,
 * and constraints — so the schedule cannot silently drift from what every
 * surface promises. These are pure-JVM assertions over androidx.work's spec
 * objects; no device or WorkManager runtime is involved.
 */
class RefreshSchedulerTest {
    @Test
    fun periodicTargetIsTheDocumentedFifteenMinuteBackgroundRefreshTarget() {
        assertEquals(15L, RefreshScheduler.PERIODIC_MINUTES)
    }

    @Test
    fun periodicRequestRunsAtTheTargetAgainstTheSharedWorker() {
        val request = RefreshScheduler.periodicRequest()

        assertEquals(
            "the periodic interval must match the documented 15-minute target",
            TimeUnit.MINUTES.toMillis(RefreshScheduler.PERIODIC_MINUTES),
            request.workSpec.intervalDuration,
        )
        assertEquals(
            "the schedule must run the shared durable refresh worker",
            WidgetRefreshWorker::class.java.name,
            request.workSpec.workerClassName,
        )
        assertTrue(
            "the worker class must be a default tag so the job is identifiable",
            request.tags.contains(WidgetRefreshWorker::class.java.name),
        )
    }

    @Test
    fun periodicRequestCarriesNoConstraints() {
        val constraints = RefreshScheduler.periodicRequest().workSpec.constraints

        assertEquals(
            "best-effort means refresh opportunities even offline: a failed fetch " +
                "keeps the last-known readings, so there is nothing to gain from " +
                "waiting for connectivity",
            NetworkType.NOT_REQUIRED,
            constraints.requiredNetworkType,
        )
        assertFalse(constraints.requiresCharging())
        assertFalse(constraints.requiresBatteryNotLow())
        assertFalse(constraints.requiresDeviceIdle())
        assertFalse(constraints.requiresStorageNotLow())
    }
}
