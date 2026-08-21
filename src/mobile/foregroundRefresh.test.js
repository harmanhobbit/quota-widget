import { describe, it, expect, vi, afterEach } from 'vitest';
import { foregroundRefresh, MIN_FOREGROUND_INTERVAL_SECS } from './foregroundRefresh.js';

// The foreground refresh cadence (issue #111). Fake timers stand in for wall
// clock so the "once on entry, then every interval, stop on leave" contract
// (CONTEXT.md "Foreground refresh") is asserted deterministically without an
// emulator.

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function controller(over = {}) {
  const refresh = over.refresh ?? vi.fn();
  const ctrl = foregroundRefresh({
    refresh,
    intervalSecs: over.intervalSecs ?? (() => 60),
  });
  return { ctrl, refresh };
}

describe('foregroundRefresh', () => {
  it('refreshes once immediately on entry', () => {
    vi.useFakeTimers();
    const { ctrl, refresh } = controller();
    ctrl.enter();
    expect(refresh).toHaveBeenCalledTimes(1);
  });

  it('repeats at the configured interval while visible', () => {
    vi.useFakeTimers();
    const { ctrl, refresh } = controller({ intervalSecs: () => 60 });
    ctrl.enter(); // 1: immediate
    vi.advanceTimersByTime(60_000);
    vi.advanceTimersByTime(60_000);
    expect(refresh).toHaveBeenCalledTimes(3); // immediate + two ticks
  });

  it('stops polling when the app leaves the foreground', () => {
    vi.useFakeTimers();
    const { ctrl, refresh } = controller();
    ctrl.enter();
    ctrl.leave();
    vi.advanceTimersByTime(10 * 60_000);
    // Only the immediate refresh ran; no ticks after leaving, and no final
    // refresh on the way out (a backgrounded app must not fetch).
    expect(refresh).toHaveBeenCalledTimes(1);
    expect(ctrl.running).toBe(false);
  });

  it('re-entering restarts the cadence rather than stacking intervals', () => {
    vi.useFakeTimers();
    const { ctrl, refresh } = controller();
    ctrl.enter(); // 1
    vi.advanceTimersByTime(30_000); // half an interval in
    ctrl.enter(); // 2 (immediate) — restarts the clock
    vi.advanceTimersByTime(60_000); // 3 — one tick from the restart
    // Had the first interval survived, its 60s mark (at t=60s) would have added
    // a fourth call; it was cleared, so exactly three.
    expect(refresh).toHaveBeenCalledTimes(3);
  });

  it('floors the interval so it can never poll faster than the minimum', () => {
    vi.useFakeTimers();
    const { ctrl, refresh } = controller({ intervalSecs: () => 1 });
    ctrl.enter(); // immediate
    vi.advanceTimersByTime((MIN_FOREGROUND_INTERVAL_SECS - 1) * 1000);
    expect(refresh).toHaveBeenCalledTimes(1); // not yet — 1s was ignored
    vi.advanceTimersByTime(1000);
    expect(refresh).toHaveBeenCalledTimes(2); // ticked at the 15s floor
  });

  it('falls back to the floor for a missing or bogus interval', () => {
    vi.useFakeTimers();
    const { ctrl, refresh } = controller({ intervalSecs: () => undefined });
    ctrl.enter();
    vi.advanceTimersByTime(MIN_FOREGROUND_INTERVAL_SECS * 1000);
    expect(refresh).toHaveBeenCalledTimes(2);
  });
});
