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
# lands, and that its results reach the surfaces that render them:
#
# 1. Startup scheduling: the app's Rust setup calls RefreshScheduler
#    .ensurePeriodic over JNI; a WorkManager job for this package must exist
#    in JobScheduler afterwards, at the ~15-minute target — and it must be
#    the ONLY one at that point, so the assertion cannot pass on some other
#    job. The methods must stay @JvmStatic for that call to find them at all
#    — without it the JNI lookup throws NoSuchMethodError, the (deliberately
#    best-effort) call degrades into the app's error log, no job ever exists,
#    and this fails.
# 2. Manual durable refresh: tapping the app's refresh button drives the real
#    refresh_manual command; its one-time WidgetRefreshWorker request carries
#    the unique tag `quota-widget-manual-refresh`, and the worker logs its own
#    tags the moment WorkManager executes it. That log line — cumulative
#    history in logcat, immune to a fast fetch closing the job's observable
#    window — is how the manual work is identified, never by counting
#    arbitrary jobs.
# 3. Worker→webview delivery: with the CI-seeded foreground interval silenced
#    (one hour — nothing else can refresh mid-check), the card's data age
#    advances to "1m ago" before the tap; the tap's worker persists and emits
#    the read model to the open webview, and the card re-renders at "just
#    now". The emit failure path is logged on the Rust side, so a silent
#    delivery failure is not an option.
#
# Widget-update scheduling (the receiver's onUpdate calling the same
# ensurePeriodic) is not automatable here: a broadcast without real widget ids
# is dropped by AppWidgetProvider, and driving launcher widget placement is
# out of scope for this script.
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
# The unique tag the manual one-time request carries (WidgetRefreshWorker
# .TAG_MANUAL); the worker logs its tags the moment WorkManager executes it.
manual_tag="quota-widget-manual-refresh"

dump_ui() {
  # uiautomator occasionally races the surface; `|| true` keeps the loop alive.
  adb shell uiautomator dump /sdcard/ui.xml >/dev/null 2>&1 || true
  adb pull /sdcard/ui.xml ui.xml >/dev/null 2>&1 || true
}

# The software emulator's Google Play services occasionally gets background-
# ANR'd under memory pressure, and the system kills our app with it — because
# it "depends on provider ...FontsProvider in dying proc
# com.google.android.gms.persistent" — a SIGKILL with no FATAL EXCEPTION in
# logcat, so the crash check below cannot see it. Every wait loop therefore
# re-checks the process and relaunches it, converting that environmental kill
# into a retry instead of a 300-second poll of a dead surface.
ensure_running() {
  if [ -z "$(adb shell pidof "$pkg" 2>/dev/null | tr -d '[:space:]')" ]; then
    echo "   (app process was killed — relaunching)"
    adb shell am start -n "$activity" >/dev/null 2>&1 || true
    sleep 5
  fi
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
  ensure_running
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
# Not "the call didn't crash" — the job itself, and exactly one of it (the only
# schedule enqueued at startup is the periodic one, so a count above one would
# already mean something else is scheduling work). Polls because WorkManager's
# JobScheduler registration can trail the enqueue by a beat. Whatever the
# outcome, the matching JOB blocks are printed so a failure carries its own
# evidence in the log.
echo "==> Asserting the periodic refresh job is scheduled (issue #111)"
periodic=0
deadline=$(( SECONDS + 30 ))
while [ "$SECONDS" -lt "$deadline" ]; do
  ensure_running
  adb shell dumpsys jobscheduler > jobs.txt 2>/dev/null || true
  if grep -q "$wm_component" jobs.txt; then
    periodic=1
    break
  fi
  sleep 2
done
job_count=$(grep -c "JOB #.*$wm_component" jobs.txt || true)
echo "   scheduled jobs after startup: $job_count"
grep -A30 "JOB #.*$wm_component" jobs.txt | head -80 || true
if [ "$periodic" -ne 1 ]; then
  echo "!! No WorkManager job for $pkg — RefreshScheduler.ensurePeriodic did not land."
  exit 1
fi
if [ "${job_count:-0}" -ne 1 ]; then
  echo "!! Expected exactly one scheduled job (the periodic refresh), found $job_count."
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

# ---- 2. Manual refresh: durable work executes, then reaches the webview -----
#
# First, establish a stale baseline. The CI-seeded foreground interval is one
# hour (see mobile.rs's ci_test_key seed), so nothing refreshes mid-check and
# the card's data age only advances. Waiting until the card reports its age in
# minutes makes the post-tap reset attributable to exactly one thing: the
# worker's emit. Without this, the foreground loop could have refreshed a
# second ago and the age would already read "just now".
echo "==> Waiting for a stale baseline (card age in minutes)"
stale=0
deadline=$(( SECONDS + 240 ))
while [ "$SECONDS" -lt "$deadline" ]; do
  ensure_running
  dump_ui
  if grep -qiE "[0-9]+m ago" ui.xml; then
    stale=1
    break
  fi
  # Transient system ANR dialogs occlude the app; dismiss as during render.
  if grep -qi "isn't responding\|not responding" ui.xml; then
    echo "   (dismissing an ANR dialog)"
    adb shell input keyevent KEYCODE_BACK >/dev/null 2>&1 || true
  fi
  sleep 4
done
if [ "$stale" -ne 1 ]; then
  echo "!! The card never aged past 'just now' — the delivery assertion below"
  echo "   could not attribute a refresh to the manual tap."
  exit 1
fi
echo "   stale baseline reached"

echo "==> Tapping the manual refresh button and asserting durable work + delivery"
dump_ui
bounds=$(grep -o "<node[^>]*text=\"$refresh_glyph\"[^>]*bounds=\"\[[0-9]*,[0-9]*\]\[[0-9]*,[0-9]*\]\"" ui.xml | head -1 | grep -o "\[[0-9]*,[0-9]*\]\[[0-9]*,[0-9]*\]")
if [ -z "$bounds" ]; then
  echo "!! Refresh button ($refresh_glyph) not found in the UI hierarchy:"
  cat ui.xml
  exit 1
fi
coords=$(printf '%s' "$bounds" | sed -E 's/\[([0-9]+),([0-9]+)\]\[([0-9]+),([0-9]+)\]/\1 \2 \3 \4/')
read -r left top right bottom <<< "$coords"
adb shell input tap $(( (left + right) / 2 )) $(( (top + bottom) / 2 ))

# (a) The durable work executed under WorkManager. Identified by the manual
# request's unique tag in the worker's own execution log — logcat is cumulative
# history, so a fast-failing fetch cannot fall between samples the way a
# JobScheduler poll could.
ran=0
deadline=$(( SECONDS + 30 ))
while [ "$SECONDS" -lt "$deadline" ]; do
  ensure_running
  if adb logcat -d 2>/dev/null | grep -q "QuotaWidgetRefresh.*$manual_tag"; then
    ran=1
    break
  fi
  sleep 0.5
done
if [ "$ran" -ne 1 ]; then
  echo "!! No manual durable work ran after the tap (worker log tag: $manual_tag)."
  echo "   App-side Rust log — did the command run, and did the enqueue succeed?"
  adb logcat -d 2>/dev/null | grep "RustStdoutStderr" | grep "\[mobile\]" | tail -10 || true
  exit 1
fi
echo "   durable manual refresh work executed under WorkManager"

# (b) Delivery to the open webview. The worker persists the read model and
# pushes `snapshots` to the runtime; with the foreground loop silenced, the
# card's age resetting to "just now" is attributable to that push and nothing
# else — the UI re-rendered from the worker's emit.
fresh=0
deadline=$(( SECONDS + 90 ))
while [ "$SECONDS" -lt "$deadline" ]; do
  ensure_running
  dump_ui
  if grep -qi "just now" ui.xml; then
    fresh=1
    break
  fi
  if grep -qi "isn't responding\|not responding" ui.xml; then
    echo "   (dismissing an ANR dialog)"
    adb shell input keyevent KEYCODE_BACK >/dev/null 2>&1 || true
  fi
  sleep 2
done
if [ "$fresh" -ne 1 ]; then
  echo "!! The card never returned to 'just now' — the worker's snapshots did"
  echo "   not reach the open webview. Rust-side emit failures would appear here:"
  adb logcat -d 2>/dev/null | grep "RustStdoutStderr" | grep "\[worker\]" | tail -10 || true
  exit 1
fi
echo "   open webview re-rendered from the worker's emit (age reset)"

echo "==> OK: background refresh verified (schedule, durable manual work, webview delivery)."
