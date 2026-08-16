# Share the domain and foreground UI with a native Android host

Quota Widget's Android form is a standalone full app plus independently
configured home-screen widgets, promising outcome parity rather than desktop
interaction parity. It stays in this repository: a portable `quota-core` owns
provider, quota, ordering and alert behaviour; the foreground Tauri app reuses
Svelte components; and a native Android host owns Glance widgets, WorkManager,
Keystore-backed secrets, notifications and the JNI boundary into Rust. This
keeps the behaviour users rely on shared without pretending a launcher-hosted
widget can render Svelte or that Android can run the permanent desktop poller.

## Considered options

A single Tauri shell with desktop and Android branches throughout was rejected
because tray/window lifecycle and Android activity/widget lifecycle are
different hosts, not small platform variations. A separate native rewrite was
rejected because it would duplicate provider semantics and let the two products
drift.

## Consequences

Android schedules best-effort background refreshes targeting fifteen minutes
and persists snapshots for every widget instance; manual refresh remains
available, and there is no permanent foreground service. Credentials are
encrypted under an app-only Android Keystore key that background work may use,
are excluded from backup, and pending built-in sign-ins are persisted only
until expiry. Shared configuration is separated from platform preferences.

Provider availability does not imply identical credential sources: Android
uses direct HTTPS and pasted keys, built-in sign-in for Claude and Codex, and
cookie-only Hermes; desktop CLI files, local commands, SSH and Tailscale are not
Android capabilities. The first personal Android build is a consistently
signed, manually dispatched APK rather than a public release or Play Store
artifact.

The Android app starts with provider onboarding rather than desktop's
CLI-oriented default accounts. It refreshes immediately and periodically only
while visible; otherwise the native scheduler owns refresh opportunities. Both
the app and widgets retain visibly stale last-known readings after a failure.
Pending sign-ins survive activity/process loss until expiry, and failure to
persist a rotated credential is an authentication/storage failure rather than
a successful refresh.

Android automatic backup is disabled: configuration, secrets, pending sign-ins,
snapshots, alert memory and widget preferences all belong to one installation.
Alert memory survives ordinary process death, reboot and upgrade, while derived
snapshot corruption is discarded and user-authored configuration is preserved
and blocks replacement until the user exports or explicitly replaces it. A
widget whose selected account disappears becomes unconfigured rather than
choosing a replacement.

The initial compatibility boundary is API 24 and `arm64-v8a`, built with NDK 28
or newer; emulator builds may additionally target `x86_64`. The app is
phone-first and widgets are responsive, but tablets and foldables are not an
initial validation promise.

Every personal build uses the permanent Android application identity
`tech.allaway.quotawidget` and a stable signing key held by GitHub Actions.
Workspace SemVer supplies `versionName`; `versionCode` is
`major * 1,000,000 + minor * 1,000 + patch`, while branch and commit identity
remain build metadata rather than versions. A manually dispatched workflow
builds a signed `arm64-v8a` APK from a chosen ref and retains it, its checksum
and build metadata as Actions artifacts without publishing a release or update
manifest. Actual-device validation targets a Google Pixel 7 running Android 17
with Pixel Launcher.
