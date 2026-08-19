import { describe, it, expect } from 'vitest';
import { periodProgress } from './period.js';

// Local-time instants so the assertions hold in any timezone: the function keys
// off the device's local calendar, and these helpers build local dates and
// round-trip them through ISO strings the way the Rust model serializes
// DateTime<Utc>.
const at = (y, m, d, h = 0) => new Date(y, m - 1, d, h).getTime();
const iso = (ms) => new Date(ms).toISOString();

// 2025-01-06 is a Monday, so a Mon–Sun week is the 6th through the 12th.
const weekly = (start, end) => ({
  metric_id: 'weekly',
  period_start: iso(start),
  resets_at: iso(end),
});

const monFri = {
  monday: true,
  tuesday: true,
  wednesday: true,
  thursday: true,
  friday: true,
  saturday: false,
  sunday: false,
};
const allSeven = {
  monday: true,
  tuesday: true,
  wednesday: true,
  thursday: true,
  friday: true,
  saturday: true,
  sunday: true,
};

describe('periodProgress with a usage schedule', () => {
  it('paces a Mon–Fri schedule across working days', () => {
    // Monday 00:00 → next Monday 00:00. Wednesday noon is halfway through the
    // five working days: two full days (Mon, Tue) plus half of Wednesday.
    const w = weekly(at(2025, 1, 6), at(2025, 1, 13));
    expect(periodProgress(w, at(2025, 1, 8, 12), monFri)).toBeCloseTo(0.5, 6);
    // Each active day advances the marker by a fifth.
    expect(periodProgress(w, at(2025, 1, 6, 12), monFri)).toBeCloseTo(0.1, 6);
    expect(periodProgress(w, at(2025, 1, 7, 12), monFri)).toBeCloseTo(0.3, 6);
  });

  it('freezes across an off-day', () => {
    const w = weekly(at(2025, 1, 6), at(2025, 1, 13));
    // By Friday's end all five working days have elapsed, so the marker sits at
    // 100% and holds flat across the weekend instead of creeping towards Monday.
    expect(periodProgress(w, at(2025, 1, 11), monFri)).toBeCloseTo(1, 6); // Sat 00:00
    expect(periodProgress(w, at(2025, 1, 11, 12), monFri)).toBeCloseTo(1, 6); // Sat noon
    expect(periodProgress(w, at(2025, 1, 12, 12), monFri)).toBeCloseTo(1, 6); // Sun noon
    // …and it only reached 100% once Friday had fully elapsed.
    expect(periodProgress(w, at(2025, 1, 10, 18), monFri)).toBeCloseTo(0.95, 6); // 4.75/5
  });

  it('treats an all-seven schedule as the raw calendar', () => {
    const w = weekly(at(2025, 1, 6), at(2025, 1, 13));
    const now = at(2025, 1, 8, 12);
    expect(periodProgress(w, now, allSeven)).toBe(periodProgress(w, now));
    // And that is the raw calendar position: 2.5 days into a 7-day week.
    expect(periodProgress(w, now, allSeven)).toBeCloseTo(2.5 / 7, 6);
  });

  it('ignores the schedule for a non-weekly window', () => {
    const start = at(2025, 1, 6, 8);
    const fiveHour = {
      metric_id: 'five_hour',
      period_start: iso(start),
      resets_at: iso(start + 5 * 3_600_000),
    };
    expect(periodProgress(fiveHour, start + 2 * 3_600_000, monFri)).toBeCloseTo(0.4, 6);
    expect(periodProgress(fiveHour, start + 2 * 3_600_000, monFri)).toBe(
      periodProgress(fiveHour, start + 2 * 3_600_000),
    );

    const monthly = {
      metric_id: 'monthly_cap',
      period_start: iso(start),
      resets_at: iso(start + 30 * 86_400_000),
    };
    expect(periodProgress(monthly, start + 15 * 86_400_000, monFri)).toBeCloseTo(0.5, 6);
  });

  it('paces a period that starts mid-week', () => {
    // Thursday noon → next Thursday noon. The weekend falls mid-period, and the
    // boundary Thursdays are half-days, so the total is still five working days.
    const w = weekly(at(2025, 1, 9, 12), at(2025, 1, 16, 12));
    // Friday noon: half of Thursday + half of Friday = one working day.
    expect(periodProgress(w, at(2025, 1, 10, 12), monFri)).toBeCloseTo(0.2, 6);
    // Friday end (Sat 00:00): half of Thu + all of Fri = 1.5 working days.
    expect(periodProgress(w, at(2025, 1, 11), monFri)).toBeCloseTo(0.3, 6);
    // Held flat across the off-days, then Monday noon resumes the pace.
    expect(periodProgress(w, at(2025, 1, 12, 12), monFri)).toBeCloseTo(0.3, 6);
    expect(periodProgress(w, at(2025, 1, 13, 12), monFri)).toBeCloseTo(0.4, 6);
  });

  it('counts a partial boundary day fractionally', () => {
    const w = weekly(at(2025, 1, 9, 12), at(2025, 1, 16, 12));
    // Six hours into a half-day boundary Thursday: 0.25 of five working days,
    // not a whole day (which would read 0.2).
    expect(periodProgress(w, at(2025, 1, 9, 18), monFri)).toBeCloseTo(0.05, 6);
  });

  it('falls back to the raw calendar for an absent or unrecognised metric_id', () => {
    const start = at(2025, 1, 6);
    const end = at(2025, 1, 13);
    const now = at(2025, 1, 8, 12);
    const noId = { period_start: iso(start), resets_at: iso(end) };
    const mystery = { metric_id: 'mystery_window', period_start: iso(start), resets_at: iso(end) };
    expect(periodProgress(noId, now, monFri)).toBe(periodProgress(noId, now));
    expect(periodProgress(mystery, now, monFri)).toBe(periodProgress(mystery, now));
    expect(periodProgress(noId, now, monFri)).toBeCloseTo(2.5 / 7, 6);
  });

  it('stays within [0, 1] at and past the reset', () => {
    const w = weekly(at(2025, 1, 6), at(2025, 1, 13));
    expect(periodProgress(w, at(2025, 1, 20, 12), monFri)).toBe(1); // past reset
    expect(periodProgress(w, at(2025, 1, 13), monFri)).toBe(1); // exactly at reset
    expect(periodProgress(w, at(2025, 1, 1, 12), monFri)).toBe(0); // before start
  });
});
