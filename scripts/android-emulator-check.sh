#!/usr/bin/env bash
#
# Post-boot verification for the Android emulator proof (issue #108).
#
# WHY THIS IS A FILE AND NOT AN INLINE `script:` BLOCK
# ----------------------------------------------------
# reactivecircus/android-emulator-runner executes the `script` input
# line-by-line, each line in its own `sh -c`. Any multi-line shell construct
# (an `if … then … fi`, a `while` loop) is therefore split across invocations
# and the first line alone is handed to the shell — which fails with
# "Syntax error: end of file unexpected (expecting \"fi\")" and aborts the job
# before a single assertion runs. Collapsing the logic into one file that the
# `script:` calls as a SINGLE line sidesteps that entirely, and lets the logic
# be syntax-checked locally (`bash -n`) instead of only in a 20-minute CI run.
#
# WHAT IT PROVES
# --------------
# That the Svelte frontend actually mounts and renders inside Tauri's Android
# WebView — the "OpenRouter" provider label appears in the live UI hierarchy.
# A parse/runtime failure in the bundle (e.g. the Chrome-69 `globalThis` gap)
# leaves the WebView blank, so the label never appears and this fails.
#
# The emulator runs under pure software emulation on GitHub-hosted Linux
# runners (no KVM), so first paint is slow (~20-40s) and the system throws
# transient "System UI isn't responding" ANR dialogs that occlude the app.
# Hence: poll with a generous budget rather than a single fixed sleep, and
# dismiss ANR dialogs as they appear instead of screenshotting through them.
set -euo pipefail

apk="${1:?usage: android-emulator-check.sh <path-to-apk>}"
pkg="tech.allaway.quotawidget"
activity="${pkg}/.MainActivity"

# Total budget for the app to boot and render, in seconds. Deliberately
# generous because the runner has no hardware acceleration. Overridable via
# ANDROID_CHECK_BUDGET so the polling logic can be exercised quickly in a
# local mock-adb test without waiting the full CI budget.
deadline=$(( SECONDS + ${ANDROID_CHECK_BUDGET:-240} ))

dump_ui() {
  # uiautomator occasionally races the surface; `|| true` keeps the loop alive.
  adb shell uiautomator dump /sdcard/ui.xml >/dev/null 2>&1 || true
  adb pull /sdcard/ui.xml ui.xml >/dev/null 2>&1 || true
}

echo "==> Installing APK: $apk"
adb install -r "$apk"

echo "==> Launching $activity"
adb shell am start -n "$activity"

echo "==> Waiting for the OpenRouter card to render (budget: ${ANDROID_CHECK_BUDGET:-240}s)"
found=0
while [ "$SECONDS" -lt "$deadline" ]; do
  # Bail out immediately on a genuine crash — no point polling a dead process.
  if adb logcat -d 2>/dev/null | grep -q "FATAL EXCEPTION"; then
    echo "!! FATAL EXCEPTION in logcat — app crashed on launch"
    break
  fi

  dump_ui
  if [ -f ui.xml ]; then
    # A blank WebView from a bundle that threw before rendering will never
    # contain this; a mounted Svelte app always does (settings <h2> or the
    # rendered UsageCard provider name).
    if grep -qi "OpenRouter" ui.xml; then
      found=1
      break
    fi
    # The software-emulated system throws transient ANR dialogs that sit on
    # top of the app. Dismissing with BACK selects the dialog's default
    # ("Wait") and lets the app surface come forward on the next poll.
    if grep -qi "isn't responding\|not responding" ui.xml; then
      echo "   (dismissing an ANR dialog)"
      adb shell input keyevent KEYCODE_BACK >/dev/null 2>&1 || true
    fi
  fi
  sleep 5
done

echo "==> Capturing diagnostics"
adb exec-out screencap -p > emulator-screenshot.png 2>/dev/null || true
adb logcat -d > logcat.txt 2>/dev/null || true
dump_ui

if [ "$found" -ne 1 ]; then
  echo "!! OpenRouter card did not render within the budget."
  echo "   Console errors seen (if any):"
  grep -i "Tauri/Console" logcat.txt 2>/dev/null | tail -20 || true
  exit 1
fi

# Even on success, surface any console errors so a rendered-but-degraded state
# (e.g. a failed network refresh) is visible in the log without failing the job.
if grep -qi "Tauri/Console.*Error" logcat.txt 2>/dev/null; then
  echo "   note: console errors were logged despite a successful render:"
  grep -i "Tauri/Console" logcat.txt | tail -20
fi

echo "==> OK: OpenRouter card rendered in the Android WebView."
