# Android preview channel (Obtainium)

A way to put the Android build on a real phone and keep it updated, without a
real signed release. It is a **testing channel**: arm64, **debug-signed** with a
public throwaway key, published as a rolling GitHub **prerelease** tagged
`android-preview`. It does not touch the signed release channel (`release.yml`),
`latest.json`, the desktop updater, or the app version.

Workflow: [`.github/workflows/android-preview.yml`](../.github/workflows/android-preview.yml).
Signing key: [`ci/android-preview/`](../ci/android-preview/README.md).

## Cutting a preview build

The workflow is **dispatch-only**. Because `workflow_dispatch` only lists a
workflow once its file is on the default branch, this becomes runnable after the
file lands on `main`.

```sh
# Builds whatever ref you pass and (re)publishes the `android-preview` prerelease.
gh workflow run android-preview.yml --ref main
```

- The Android app code must be on the ref you build. Until the Android feature
  branch merges, dispatch it against that branch
  (`--ref feat/android-foreground-proof`) — the workflow file still has to exist
  on `main` for dispatch to be allowed, but the build uses the ref you name.
- Each run gets a unique `versionName` (`<version>-preview.<run>`) and an
  incrementing `versionCode` (the run number), and the prerelease is
  delete-and-recreated so its timestamp is fresh. That is what lets Android
  accept the update and Obtainium notice it, even though the tag never changes
  and the app version is never bumped.

## Installing with Obtainium

1. Install [Obtainium](https://github.com/ImranR98/Obtainium) (itself installable
   from its GitHub releases).
2. **Add App** → App Source URL:

   ```
   https://github.com/harmanhobbit/quota-widget
   ```
3. In the source options before adding:
   - Enable **Include prerelease versions** (the channel is a prerelease).
   - Enable **Use release date as version** (the tag is rolling, so the version
     string doesn't change — the fresh release timestamp is what marks each
     build as newer). If you'd rather track the APK, Obtainium can instead read
     the `versionName` from the APK; either works.
   - Leave APK filtering alone — the release ships a single
     `quota-widget-android-arm64.apk`.
4. **Add**, then install when Obtainium prompts. Android will warn about an app
   from an unknown source and about a debuggable build — expected.
5. On first launch, open **Settings** (⚙) and add an account. The preview APK
   ships with **no** key baked in — embedding one would leak it, since the
   prerelease is public. Supported providers on Android:
   - **Claude** and **Codex** through built-in browser/device-flow sign-in.
   - **Hermes Portal** by pasting a `portal.nousresearch.com` session cookie.
   - Every direct-HTTPS provider by pasting its API key (OpenRouter, ElevenLabs,
     Firecrawl, DeepSeek, Moonshot, Venice, OneHop, Fireworks, Anthropic Admin,
     OpenAI Admin).
   Desktop credential sources (CLI files, local commands, SSH, Tailscale) are
   not exposed on Android.

## Updating

Cut a new preview build (dispatch the workflow again), then in Obtainium pull to
refresh — it will offer the update in place. Because every build is signed with
the same committed debug key, the update installs over the old one without an
uninstall.

## Caveats

- **arm64 only.** Fine for essentially every modern phone; it will not install
  on an x86 device or the CI emulator. Change `--target aarch64` to build wider
  if you ever need to.
- **Debug-signed, debuggable, public key.** This is a preview, not a release.
  Anyone can sign an APK with the same public debug key, so do not treat an
  Obtainium "update" here as trusted-origin. A real signed release channel is a
  later ticket (`docs/adr/0006-…`).
- **No notifications yet** — threshold alerts are computed and their memory
  persisted (issue #112's seam), but Android does not post them; presenting
  alerts as notifications is a later ticket. Background refresh is in
  (issue #111: a best-effort WorkManager job targeting ~15 minutes while the
  app is unused), and home-screen widgets render the last-known data
  (issue #113).
