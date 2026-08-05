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

// How far through the window's period we are, 0–1, or null when the provider
// couldn't tell us the period's bounds. Drawn against the usage bar so a
// half-full bar at the quarter mark reads as "burning it fast".
export function periodProgress(w, now) {
  if (!w.resets_at || !w.period_start) return null;
  const start = new Date(w.period_start).getTime();
  const end = new Date(w.resets_at).getTime();
  const span = end - start;
  if (!(span > 0)) return null;
  return Math.min(Math.max((now - start) / span, 0), 1);
}

// The period marker's tooltip. It names what the marker *is* — the mini
// summary has no legend and no countdown text — and pairs it with the reset
// time, which appears nowhere else on that surface.
export function periodTooltip(w, progress, now) {
  return `${Math.round(progress * 100)}% through · ${resetsIn(w.resets_at, now)}`;
}
