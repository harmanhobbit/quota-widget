# Native Glance home-screen widget host (issue #113)

This directory is the **committed source of truth** for the Android home-screen
widget's native host. The Gradle project it compiles into —
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

## Layout

| Path | Role |
|------|------|
| `kotlin/.../widget/WidgetBridge.kt` | `external` JNI declarations into `libquota_widget_lib.so`. |
| `kotlin/.../widget/WidgetModel.kt` | Parse the `WidgetView` wire JSON into typed Kotlin (no decisions). |
| `kotlin/.../widget/QuotaGlanceWidget.kt` | The Glance widget: small/medium/large tiers, status colours + non-colour cues, deep-link and refresh actions. |
| `kotlin/.../widget/QuotaWidgetReceiver.kt` | `GlanceAppWidgetReceiver` + the refresh `ActionCallback`. |
| `kotlin/.../widget/WidgetRefreshWorker.kt` | The one-time durable WorkManager job the refresh action enqueues. |
| `kotlin/.../widget/WidgetConfigActivity.kt` | Placement configuration (per-instance accounts + privacy). |
| `kotlin/.../widget/WidgetPaths.kt` | The app config directory both the app and the widget read. |
| `res/xml/quota_glance_widget_info.xml` | The `appwidget-provider` metadata (config activity, resize, sizes). |
| `test/.../widget/WidgetModelTest.kt` | JVM unit tests for the wire-format parse — also a drift guard on the Rust DTO. |

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
