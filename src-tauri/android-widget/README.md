# Native Glance home-screen widget host (issues #113, #111)

This directory is the **committed source of truth** for the Android home-screen
widget's native host, and (since #111) for the app-wide background refresh
schedule: the same host owns WorkManager, so the periodic refresh lives beside
the widget code it serves. The Gradle project it compiles into —
`src-tauri/gen/android` — is generated fresh by `tauri android init` on every
build and is `.gitignore`d, so (exactly like the manifest backup-exclusion in
`scripts/patch-android-manifest.mjs`) the host is reapplied after init by
[`scripts/patch-android-glance-widget.mjs`](../../scripts/patch-android-glance-widget.mjs),
which the two Android CI jobs run right after `tauri android init`.

## Why a native host at all

Per [ADR-0006](../../docs/adr/0006-share-the-domain-and-foreground-ui-with-a-native-android-host.md),
`quota-core` owns behaviour and the host only draws. A launcher-hosted widget
cannot render Svelte, and it must render from persisted snapshots with no app
running — so the widget is native Kotlin/Glance. It makes **no quota decision**:
every projection, breakpoint, privacy redaction, removed-account and aggregate
decision is settled in `quota-core::widget` and flattened by
`quota-core::widget_view`, reached through the JNI seam in
`src-tauri/src/widget_jni.rs`. The Kotlin here parses that finished JSON and
lays it out.

## The background refresh schedule (issue #111)

`RefreshScheduler` owns the best-effort periodic refresh: one unique
`PeriodicWorkRequest` targeting fifteen minutes (`quota_core::refresh::
BACKGROUND_REFRESH_TARGET` — a target, never a guarantee, everywhere it is
described). It runs the same `WidgetRefreshWorker` the manual paths enqueue, so
every refresh — scheduled, widget-tapped, or pressed in the app — fetches
through `WidgetBridge.nativeRefresh` into the shared `quota-core` refresh and
persists the same read model. The schedule is (re)ensured from two places, both
idempotent under unique work with KEEP: the app's Rust setup (via the JNI call
in `src-tauri/src/android_schedule.rs`) and the widget receiver's `onUpdate`,
so a widget keeps refreshing even if the app is never opened again.

The two entry points the Rust side reaches (`ensurePeriodic`, `enqueueOneTime`)
are `@JvmStatic`: they are called with JNI `CallStaticVoidMethod` on the class,
and a Kotlin `object`'s plain members are instance methods only — without the
annotation the lookup fails and the best-effort call silently schedules
nothing.

## Verifying the schedule

The dispatch-only Android job exercises the real paths on the emulator
(`scripts/android-emulator-check.sh`): after the UI render proof it asserts a
WorkManager job for the app package exists in `dumpsys jobscheduler` at the
15-minute interval (the startup dispatch), then taps the app's refresh button
and asserts a second, one-time job appears on top of the schedule (the manual
durable enqueue). Those two assertions are what pin the `@JvmStatic`
requirement — a regression there schedules nothing, visibly. The
`RefreshSchedulerTest` JVM tests additionally pin the interval, worker class
and constraint-free request configuration without a device.

## Layout

| Path | Role |
|------|------|
| `kotlin/.../widget/WidgetBridge.kt` | `external` JNI declarations into `libquota_widget_lib.so`. |
| `kotlin/.../widget/WidgetModel.kt` | Parse the `WidgetView` wire JSON into typed Kotlin (no decisions). |
| `kotlin/.../widget/QuotaGlanceWidget.kt` | The Glance widget: small/medium/large tiers, status colours + non-colour cues, deep-link and refresh actions. |
| `kotlin/.../widget/QuotaWidgetReceiver.kt` | `GlanceAppWidgetReceiver` + the refresh `ActionCallback`; also ensures the periodic refresh exists on widget updates. |
| `kotlin/.../widget/WidgetRefreshWorker.kt` | The one-time durable WorkManager refresh job — enqueued by the widget's refresh action, the app's manual refresh, and the periodic schedule. |
| `kotlin/.../widget/RefreshScheduler.kt` | The background refresh schedule (issue #111): the best-effort periodic job targeting ~15 minutes, plus the app's manual-refresh enqueue that the JNI call in `src-tauri/src/android_schedule.rs` reaches. |
| `kotlin/.../widget/WidgetConfigActivity.kt` | Placement configuration (per-instance accounts + privacy). |
| `kotlin/.../widget/WidgetPaths.kt` | The app config directory both the app and the widget read. |
| `res/xml/quota_glance_widget_info.xml` | The `appwidget-provider` metadata (config activity, resize, sizes). |
| `test/.../widget/WidgetModelTest.kt` | JVM unit tests for the wire-format parse — also a drift guard on the Rust DTO. |
| `test/.../widget/RefreshSchedulerTest.kt` | Pins the periodic target to the documented fifteen minutes (`quota_core::refresh::BACKGROUND_REFRESH_TARGET`). |

## Verifying

The Kotlin/Gradle layer is **not** on the per-push CI gate (which is Linux
Rust-core + frontend, deliberately kept off Android — see `AGENTS.md`). It is
compiled and tested by the dispatch-only Android jobs:

```sh
# Compile + run the widget host's JVM unit tests, and build the debug APK:
gh workflow run build.yml --ref <branch> -f target=android
# Install on a phone via the Obtainium preview channel:
gh workflow run android-preview.yml --ref <branch>
```

Locally (with the Android SDK + NDK 28 installed), the same steps are:

```sh
npm run build
npm run tauri android init
node scripts/patch-android-manifest.mjs
node scripts/patch-android-glance-widget.mjs
( cd src-tauri/gen/android && ./gradlew :app:testDebugUnitTest )
```
