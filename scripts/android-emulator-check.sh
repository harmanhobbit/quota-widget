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
# Since issue #111 it also proves the background refresh dispatch actually
# lands, on both of its Rust→Kotlin JNI paths:
#
# 1. Startup scheduling: the app's Rust setup calls RefreshScheduler
#    .ensurePeriodic over JNI; a WorkManager job for this package must exist
#    in JobScheduler afterwards, at the ~15-minute target. The methods must
#    stay @JvmStatic for that call to find them at all — without it the JNI
#    lookup throws NoSuchMethodError, the (deliberately best-effort) call
#    degrades into the app's error log, no job ever exists, and this fails.
# 2. Manual durable enqueue: tapping the app's refresh button drives the real
#    refresh_manual command; its one-time WidgetRefreshWorker job must appear
#    in JobScheduler (durable work, not an in-process fetch). The worker
#    class name in the dump is what distinguishes it from the periodic job.
#
# Widget-update scheduling (the receiver's onUpdate calling the same
# ensurePeriodic) is not automatable here: a broadcast without real widget ids
# is dropped by AppWidgetProvider, and driving launcher widget placement is
# out of scope for this script. The worker→webview snapshots delivery is
# likewise not asserted: while the app is open the foreground loop keeps the
# figures fresh, so there is no stable UI signal that would distinguish the
# worker's emit from the loop's — it shares the exact event/payload path the
# render assertion above already exercises.
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
# WorkManager wraps every request in this service, so each of our scheduled
# jobs names it. The short "quotawidget/" prefix is unique in the dump — our
# application id, here as part of the component's "package/Class" form.
wm_component="quotawidget/androidx.work.impl.background.systemjob.SystemJobService"
refresh_glyph="⟳"

dump_ui() {
  # uiautomator occasionally races the surface; `|| true` keeps the loop alive.
  adb shell uiautomator dump /sdcard/ui.xml >/dev/null 2>&1 || true
  adb pull /sdcard/ui.xml ui.xml >/dev/null 2>&1 || true
}

echo "==> Installing APK: $apk"
adb install -r "$apk"

echo "==> Launching $activity"
adb shell am start -n "$activity"

# Start the render budget HERE, not at script top: on a KVM-less runner the
# `adb install` above can itself take 8+ minutes, and `SECONDS` counts from the
# script's start — computing the deadline earlier let the install consume the
# entire budget, so the loop ran zero iterations and gave up ~4s after launch
# (before the WebView had even painted). Deliberately generous because first
# paint + the debug auto-seed's network refresh are slow under software
# emulation. Overridable via ANDROID_CHECK_BUDGET for the local mock-adb test.
budget="${ANDROID_CHECK_BUDGET:-300}"
deadline=$(( SECONDS + budget ))

echo "==> Waiting for the OpenRouter card to render (budget: ${budget}s)"
found=0
while [ "$SECONDS" -lt "$deadline" ]; do
  # Bail out immediately on a genuine crash of OUR app — no point polling a
  # dead process. Scope to our package: a real app crash logs
  # "FATAL EXCEPTION: …" followed by "Process: <pkg>, PID: …". A bare
  # "FATAL EXCEPTION" match also caught "FATAL EXCEPTION: UiAutomation" — the
  # accessibility service crashing with "Bad file descriptor" during our OWN
  # `uiautomator dump` calls — which aborted the poll ~13s in even though the
  # app had already rendered the OpenRouter card fine.
  if adb logcat -d 2>/dev/null | grep -A3 "FATAL EXCEPTION" | grep -q "Process: $pkg"; then
    echo "!! FATAL EXCEPTION for $pkg in logcat — app crashed on launch"
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
  # Space polls out: each iteration dumps the full logcat + a uiautomator
  # snapshot, and hammering that on a KVM-less emulator only feeds the ANRs.
  sleep 8
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

# ---- 1. Startup scheduling: the periodic refresh actually exists ------------
#
# Not "the call didn't crash" — the job itself. Polls because WorkManager's
# JobScheduler registration can trail the enqueue by a beat. Whatever the
# outcome, the matching JOB blocks are printed so a failure carries its own
# evidence in the log.
echo "==> Asserting the periodic refresh job is scheduled (issue #111)"
periodic=0
deadline=$(( SECONDS + 30 ))
while [ "$SECONDS" -lt "$deadline" ]; do
  adb shell dumpsys jobscheduler > jobs.txt 2>/dev/null || true
  if grep -q "$wm_component" jobs.txt; then
    periodic=1
    break
  fi
  sleep 2
done
grep "JOB #.*$wm_component" jobs.txt | head -5 || true
grep -A24 "JOB #.*$wm_component" jobs.txt | head -80 || true
if [ "$periodic" -ne 1 ]; then
  echo "!! No WorkManager job for $pkg — RefreshScheduler.ensurePeriodic did not land."
  exit 1
fi
# WorkManager 2.9 schedules periodic work as a one-shot JobInfo pointed at the
# next period (its next-schedule-time override), so the cadence shows up as the
# job's minimum latency — ~15 minutes, minus the seconds already elapsed since
# the app scheduled it — rather than a PERIOD line. Match either shape so a
# WorkManager change of strategy cannot silently pass.
if ! grep -B2 -A30 "JOB #.*$wm_component" jobs.txt | grep -qE "Minimum latency: \+1[0-5]m|PERIOD: ?900000|PERIOD: ?\+15m"; then
  echo "!! A WorkManager job exists but is not the ~15-minute periodic one"
  echo "   (expected the ~15-minute minimum latency or period in the job dump above)."
  exit 1
fi
echo "   periodic refresh job present, interval 15 minutes"

# ---- 2. Manual refresh: one durable one-time job ---------------------------
#
# Tap the header ⟳ button — the real UI path into refresh_manual — and watch
# JobScheduler for the one-time work it must enqueue. WorkManager does not tag
# JobInfos with the worker class (that lives in its own database, invisible to
# dumpsys), so the one-time request is detected as a *second* job from our
# package on top of the periodic one. The job is visible only while pending or
# running, so the tap is repeated a couple of times first: the unique-work
# REPLACE policy collapses the burst into one fetch, and the repeated
# re-enqueue widens the window in which the pending job is observable. The
# foreground visibility loop is not a confound — it refreshes in-process and
# never enqueues WorkManager work.
echo "==> Tapping the manual refresh button and asserting durable one-time work"
dump_ui
bounds=$(grep -o "<node[^>]*text=\"$refresh_glyph\"[^>]*bounds=\"\[[0-9]*,[0-9]*\]\[[0-9]*,[0-9]*\]\"" ui.xml | head -1 | grep -o "\[[0-9]*,[0-9]*\]\[[0-9]*,[0-9]*\]")
if [ -z "$bounds" ]; then
  echo "!! Refresh button ($refresh_glyph) not found in the UI hierarchy:"
  cat ui.xml
  exit 1
fi
coords=$(printf '%s' "$bounds" | sed -E 's/\[([0-9]+),([0-9]+)\]\[([0-9]+),([0-9]+)\]/\1 \2 \3 \4/')
read -r left top right bottom <<< "$coords"
tap_x=$(( (left + right) / 2 ))
tap_y=$(( (top + bottom) / 2 ))

count_jobs() {
  adb shell dumpsys jobscheduler 2>/dev/null | grep -c "JOB #.*$wm_component" || true
}
base_jobs=$(count_jobs)
echo "   scheduled jobs before the tap: $base_jobs"

# Sample fast (the dump itself takes ~0.5s on the emulator, so each sample is
# slower than its interval) and interleave with the taps: the one-time job is
# observable only while pending or running, and a fast failing fetch can close
# that window in well under a second. The unique-work REPLACE policy makes the
# burst collapse into one fetch, while the repeated re-enqueues give each
# round a fresh window to observe.
one_time=0
for _ in 1 2 3; do
  adb shell input tap "$tap_x" "$tap_y"
  deadline=$(( SECONDS + 8 ))
  while [ "$SECONDS" -lt "$deadline" ]; do
    jobs_now=$(count_jobs)
    if [ "${jobs_now:-0}" -ge $(( base_jobs + 1 )) ]; then
      one_time=1
      break
    fi
    sleep 0.3
  done
  [ "$one_time" -eq 1 ] && break
done
if [ "$one_time" -ne 1 ]; then
  echo "!! No second WorkManager job observed after tapping refresh —"
  echo "   the manual refresh did not enqueue durable work (jobs now: ${jobs_now:-?})."
  echo "   App-side log for evidence of what the tap actually did:"
  adb logcat -d 2>/dev/null | grep "RustStdoutStderr" | grep "\[mobile\]" | tail -10 || true
  exit 1
fi
echo "   durable one-time refresh job observed on top of the periodic schedule"

echo "==> OK: background refresh dispatch verified (schedule + manual durable work)."
