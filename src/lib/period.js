// Time readings shared by the full card and the mini summary. Both surfaces
// describe the same instant, so they must not drift into wording it
// differently — hence one module rather than a copy in each component.

export function resetsIn(iso, now) {
  if (!iso) return '';
  const ms = new Date(iso).getTime() - now;
  if (ms <= 0) return 'resets soon';
  const mins = Math.round(ms / 60_000);
  if (mins < 60) return `resets in ${mins}m`;
  const h = Math.floor(mins / 60);
  if (h < 48) return `resets in ${h}h ${mins % 60}m`;
  return `resets in ${Math.floor(h / 24)}d ${h % 24}h`;
}

// The weekly window's stable metric identity. The `window:` prefix seen in
// settings (tray_metric, mini_summary_metrics) is the config-selection
// namespace, not the window's own `metric_id`, which every provider reports as
// the bare `weekly`. The per-model weekly windows (`weekly_opus`, …) are
// deliberately not scheduled: the schedule is opt-in per account and reshapes
// only the headline weekly window.
const WEEKLY_METRIC_ID = 'weekly';

// Date.getDay() value for each weekday key a serialized quota-core
// `UsageSchedule` carries. getDay() numbers Sunday 0 … Saturday 6.
const WEEKDAY_KEYS = [
  ['monday', 1],
  ['tuesday', 2],
  ['wednesday', 3],
  ['thursday', 4],
  ['friday', 5],
  ['saturday', 6],
  ['sunday', 0],
];

// The active weekdays a schedule names, as a Set of getDay() values, or null
// when there is no schedule to apply: an absent schedule, every day active, or
// zero days active all mean "pace against the raw calendar".
function activeWeekdays(schedule) {
  if (!schedule) return null;
  const days = new Set();
  for (const [key, day] of WEEKDAY_KEYS) {
    if (schedule[key]) days.add(day);
  }
  return days.size === 0 || days.size === 7 ? null : days;
}

// Local midnight of the day containing `ms`.
function localMidnight(ms) {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

// Local midnight of the day after the one containing `ms`. Calendar arithmetic
// rather than adding 24h, so a DST transition doesn't shift the boundary.
function nextMidnight(ms) {
  const d = new Date(ms);
  d.setDate(d.getDate() + 1);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

// How far through the window's period we are, 0–1, or null when the provider
// couldn't tell us the period's bounds. Drawn against the usage bar so a
// half-full bar at the quarter mark reads as "burning it fast".
//
// `schedule` is the account's usage schedule (quota-core's `UsageSchedule`,
// serialized) and reshapes only a weekly window: the marker then measures
// scheduled time — the active-day span elapsed so far over the total active-day
// span in the period — so it advances only on scheduled days and holds flat on
// off-days. Every other window, and a weekly window whose schedule is all-seven
// or absent, keeps the raw calendar fraction. Omit `schedule` for the calendar
// marker (the press-and-hold peek) or for callers that predate the schedule.
export function periodProgress(w, now, schedule) {
  if (!w.resets_at || !w.period_start) return null;
  const start = new Date(w.period_start).getTime();
  const end = new Date(w.resets_at).getTime();
  const span = end - start;
  if (!(span > 0)) return null;
  const calendar = Math.min(Math.max((now - start) / span, 0), 1);

  const active = w.metric_id === WEEKLY_METRIC_ID ? activeWeekdays(schedule) : null;
  if (active === null) return calendar;

  // Sum the active-day spans inside [start, end], and inside [start, now],
  // walking the local-midnight day boundaries so an off-day contributes nothing
  // and a boundary day counts only the hours that actually fall in the period.
  let total = 0;
  let elapsed = 0;
  const nowCapped = Math.min(now, end);
  for (let day = localMidnight(start); day < end; day = nextMidnight(day)) {
    if (!active.has(new Date(day).getDay())) continue;
    const dayEnd = nextMidnight(day);
    const from = Math.max(day, start);
    total += Math.min(dayEnd, end) - from;
    elapsed += Math.max(0, Math.min(dayEnd, nowCapped) - from);
  }
  if (total <= 0) return calendar;
  return Math.min(Math.max(elapsed / total, 0), 1);
}

// The period marker's tooltip. It names what the marker *is* — the mini
// summary has no legend and no countdown text — and pairs it with the reset
// time, which appears nowhere else on that surface.
export function periodTooltip(w, progress, now) {
  return `${Math.round(progress * 100)}% through · ${resetsIn(w.resets_at, now)}`;
}
