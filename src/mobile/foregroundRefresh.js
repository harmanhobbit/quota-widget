// Foreground refresh controller (issue #111; CONTEXT.md "Foreground refresh").
//
// The Android app refreshes quota only while it is *visible*: once immediately
// when it enters the foreground, then repeating at the configured interval, and
// it stops the moment it leaves visibility. Refresh opportunities while the app
// is backgrounded are the operating system's job — the best-effort WorkManager
// "background refresh target" — never this loop's. A foreground loop that kept
// ticking after the app was hidden would be exactly the permanent poll
// docs/adr/0006-… rules out.
//
// The cadence decision is kept separate from the DOM so it is testable under
// vitest with fake timers: the caller injects `refresh` and the timer
// primitives, drives enter()/leave(), and asserts the calls. MobileApp.svelte
// wires enter()/leave() to the document's visibility events.

// Matches poller.rs's `.max(15)` floor, so the foreground cadence can never be
// set faster than the desktop poller's minimum however low the config goes.
export const MIN_FOREGROUND_INTERVAL_SECS = 15;

/**
 * @param {object} opts
 * @param {() => void} opts.refresh        Called once on entry and once per tick.
 * @param {() => number} [opts.intervalSecs] Reads the configured interval, in
 *   seconds, each time the loop (re)starts. Defaults to 60.
 * @param {typeof setInterval} [opts.setInterval] Injectable for tests.
 * @param {typeof clearInterval} [opts.clearInterval] Injectable for tests.
 */
export function foregroundRefresh({
  refresh,
  intervalSecs = () => 60,
  setInterval: setTimer = globalThis.setInterval,
  clearInterval: clearTimer = globalThis.clearInterval,
}) {
  let timer = null;

  function stop() {
    if (timer !== null) {
      clearTimer(timer);
      timer = null;
    }
  }

  return {
    // Refresh now, then repeat while visible. Idempotent: a second visibility
    // event that fires enter() again restarts the cadence from now rather than
    // stacking a second interval alongside the first.
    enter() {
      stop();
      refresh();
      const secs = Math.max(
        MIN_FOREGROUND_INTERVAL_SECS,
        Number(intervalSecs()) || MIN_FOREGROUND_INTERVAL_SECS,
      );
      timer = setTimer(() => refresh(), secs * 1000);
    },
    // Leave the foreground: stop polling entirely. No final refresh — a
    // backgrounded app must not fetch.
    leave() {
      stop();
    },
    get running() {
      return timer !== null;
    },
  };
}
