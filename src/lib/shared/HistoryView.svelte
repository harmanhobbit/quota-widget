<script>
  // Per-account [[usage history]] charts, shared by the desktop History tab
  // and the Android app so both surfaces render the same record the same way
  // ([[outcome parity]]). One inline-SVG line per [[usage window]] on a fixed
  // 0–100 percentage axis, plus a separate credits chart where the account
  // has credits — a percentage and a money amount are different quantities
  // (see CONTEXT.md "Credits") and never share an axis. Inline SVG only: no
  // charting dependency, because a package.json dependency forces an
  // npmDeps.hash regen in nix/package.nix.
  //
  // What is plotted: the figures an account actually reported. A failed
  // reading is never plotted — the figures a failure may carry were preserved
  // from an earlier success ([[stale reading]]), so charting them at record
  // time would invent a movement that never happened. A failure simply leaves
  // a gap in the line; the host's readings surface is where failures live,
  // not the chart.
  //
  // Scale and axes: a sparkline without scale reads as shape, not data, so
  // every chart carries gridlines and axis labels. The gridlines are SVG
  // strokes — the viewBox stretches non-uniformly, and `non-scaling-stroke`
  // keeps them 1px — but the tick labels are HTML around the plot, because
  // text inside a stretched viewBox would distort. The percentage scale is
  // fixed and labelled 0–100 (a usage window IS a percentage); credits
  // auto-scale to the plotted balances with the bounds pushed out to round
  // steps, so gridlines land on round numbers.
  //
  // All computation below is pure — the template reads one `$derived`, and no
  // helper writes state, so none of it can trip Svelte 5's
  // state_unsafe_mutation.
  //
  // Line colours: one source of truth per series, assigned in buildAccounts
  // and applied to BOTH the polyline stroke and its legend swatch, so the two
  // cannot disagree. (Colouring used to live in CSS as .usage-line:nth-of-type(n)
  // — keyed on rendered position, so a line dropping out of the range silently
  // recoloured its neighbours.) Assignment is by index in the account's
  // metric-id list, which is computed over ALL recorded points and is therefore
  // range-independent: a line keeps its colour as the range selector moves.
  // The palette holds mid-lightness hues so every entry reads on both the
  // light and dark themes; the credits line keeps its own distinct colour,
  // never shared with a window.
  //
  // [[usage history]]: ../../CONTEXT.md
  // [[usage window]]: ../../CONTEXT.md
  // [[chart legend]]: ../../CONTEXT.md
  // [[outcome parity]]: ../../CONTEXT.md
  // [[stale reading]]: ../../CONTEXT.md
  let { history = [], snapshots = [] } = $props();

  const SERIES_PALETTE = ['#4c9be8', '#e6a817', '#9a6dd7', '#d16ba5', '#5bbcbf', '#c86d5a'];
  const CREDITS_COLOR = '#2fa46a';

  // The selectable time ranges. `null` means unbounded ("all time").
  const RANGES = [
    { key: '24h', label: '24 hours', ms: 24 * 3600_000 },
    { key: '7d', label: '7 days', ms: 7 * 24 * 3600_000 },
    { key: '30d', label: '30 days', ms: 30 * 24 * 3600_000 },
    { key: 'all', label: 'All time', ms: null },
  ];
  let range = $state('7d');

  // Chart geometry in viewBox units. The percentage axis is fixed 0–100 (a
  // usage window IS a percentage); credits auto-scale to the plotted values.
  const H = 40;

  // The percentage scale, shared by every account's usage chart: gridlines
  // and labels for 0 / 50 / 100 percent, top label first for the HTML column
  // (which fills downward) and plot order preserved for the SVG lines.
  const PCT_TICKS = [100, 50, 0].map((pct) => ({ pct, y: pctToY(pct), label: `${pct}%` }));

  function timeToX(at, t0, t1) {
    const t = new Date(at).getTime();
    if (t1 <= t0) return 0;
    return Math.min(100, Math.max(0, ((t - t0) / (t1 - t0)) * 100));
  }

  function pctToY(pct) {
    const clamped = Math.min(100, Math.max(0, pct));
    return H - (clamped / 100) * H;
  }

  // X-axis labels for the plotted span: start, middle, end. Short dates,
  // except the 24-hour range where every date would read the same and the
  // times are the informative axis.
  function rangeLabel(t, rangeKey) {
    const d = new Date(t);
    return rangeKey === '24h'
      ? d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
      : d.toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
  }

  // A round-number axis for the credits chart: the data's bounds pushed out
  // to a "nice" step (1/2/5 × 10^k) so gridlines land on round values. A
  // flat history — one reading, or the same balance throughout — gets a
  // padded axis centred on that balance instead of a divide-by-zero.
  function niceAxis(lo, hi) {
    if (!(hi > lo)) {
      const pad = Math.max(Math.abs(hi) * 0.5, 1);
      lo -= pad;
      hi += pad;
    }
    const raw = (hi - lo) / 4;
    const mag = 10 ** Math.floor(Math.log10(raw));
    const norm = raw / mag;
    const step = (norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10) * mag;
    const axisLo = Math.floor(lo / step) * step;
    const axisHi = Math.ceil(hi / step) * step;
    const n = Math.round((axisHi - axisLo) / step);
    const ticks = [];
    for (let i = 0; i <= n; i += 1) {
      const v = axisLo + i * step;
      ticks.push({
        y: H - ((v - axisLo) / (axisHi - axisLo)) * H,
        // Round away float noise (4.000000000000001 → 4) for the label.
        label: String(Math.round(v * 100) / 100),
      });
    }
    return { lo: axisLo, hi: axisHi, ticks };
  }

  // The polyline for one window: every plotted, non-failed point that
  // carries this metric, in time order.
  function lineCoords(points, metricId, t0, t1) {
    return points
      .filter((p) => !p.failed)
      .map((p) => {
        const window = (p.windows ?? []).find(([id]) => id === metricId);
        return window
          ? `${timeToX(p.at, t0, t1).toFixed(2)},${pctToY(window[1]).toFixed(2)}`
          : null;
      })
      .filter(Boolean)
      .join(' ');
  }

  // A window's latest plotted figure: the newest in-range, non-failed point
  // carrying THIS metric — not the account's newest reading, which may lack
  // the window entirely. The legend shows the line's own end, and null when
  // the line has nothing plotted.
  function latestValue(points, metricId) {
    for (let i = points.length - 1; i >= 0; i -= 1) {
      const p = points[i];
      if (p.failed) continue;
      const window = (p.windows ?? []).find(([id]) => id === metricId);
      if (window) return window[1];
    }
    return null;
  }

  // The credits polyline, mapped onto the nice axis computed for the same
  // points — line and gridlines are one scale, so a gridline always means
  // what it says. The map keeps the line inside the plot whatever rounding
  // does; bounds come from a reduction, not spread over Math.min/max — an
  // all-time history can outgrow the argument count an engine accepts.
  function creditCoords(points, axis, t0, t1) {
    const span = axis.hi - axis.lo;
    return points
      .map((p) => {
        const y = H - ((p.credits_balance - axis.lo) / span) * H;
        return `${timeToX(p.at, t0, t1).toFixed(2)},${Math.min(H, Math.max(0, y)).toFixed(2)}`;
      })
      .join(' ');
  }

  function buildAccounts(history, snapshots, rangeKey) {
    const rangeMs = RANGES.find((r) => r.key === rangeKey)?.ms ?? null;
    const now = Date.now();
    return (history ?? []).map((account) => {
      const snap = snapshots.find((s) => s.provider_id === account.provider_id);
      const all = account.points ?? [];
      const plotted = all
        .filter((p) => rangeMs == null || now - new Date(p.at).getTime() <= rangeMs)
        .sort((a, b) => new Date(a.at) - new Date(b.at));
      const t1 = now;
      const t0 = rangeMs == null
        ? (plotted.length ? new Date(plotted[0].at).getTime() : t1 - 24 * 3600_000)
        : t1 - rangeMs;
      // The union of metric ids over ALL recorded points — not just the
      // plotted range — so a line keeps its identity and colour as the range
      // moves, even where a window is temporarily absent from the readings.
      const metricIds = [...new Set(all.flatMap((p) => (p.windows ?? []).map(([id]) => id)))];
      const windowLines = metricIds.map((id, i) => {
        const latest = latestValue(plotted, id);
        return {
          id,
          label: snap?.windows?.find((w) => w.metric_id === id)?.label ?? id,
          // One colour per series, from the window's position in the
          // account's metric list — the same value the legend swatch shows,
          // so line and swatch cannot disagree. The palette repeats only
          // past six windows on one account.
          color: SERIES_PALETTE[i % SERIES_PALETTE.length],
          // The line's own newest in-range figure, rounded to a whole
          // percentage for the legend.
          value: latest == null ? null : `${Math.round(latest)}%`,
          coords: lineCoords(plotted, id, t0, t1),
        };
      });
      const creditPoints = plotted.filter((p) => !p.failed && p.credits_balance != null);
      let credits = null;
      if (creditPoints.length) {
        const values = creditPoints.map((p) => p.credits_balance);
        const lo = values.reduce((a, b) => (b < a ? b : a));
        const hi = values.reduce((a, b) => (b > a ? b : a));
        const axis = niceAxis(lo, hi);
        const unit = snap?.credits?.unit ?? '';
        const latest = creditPoints[creditPoints.length - 1].credits_balance;
        credits = {
          unit,
          color: CREDITS_COLOR,
          axis,
          coords: creditCoords(creditPoints, axis, t0, t1),
          // The latest in-range balance, rounded against float noise the
          // same way the axis labels are, with its unit when one is known.
          value: `${Math.round(latest * 100) / 100}${unit ? ` ${unit}` : ''}`,
        };
      }
      const hasPlotted = windowLines.some((l) => l.coords) || credits !== null;
      return {
        id: account.provider_id,
        name: snap?.provider_name ?? account.provider_id,
        windowLines,
        credits,
        // The plotted span's time labels: start, middle, end — the X axis
        // every chart in this account shares.
        xLabels: [t0, (t0 + t1) / 2, t1].map((t) => rangeLabel(t, rangeKey)),
        hasPlotted,
      };
    });
  }

  let accounts = $derived(buildAccounts(history, snapshots, range));
</script>

<div class="history-view">
  <label class="history-range-label">Range
    <select class="history-range" bind:value={range}>
      {#each RANGES as r (r.key)}
        <option value={r.key}>{r.label}</option>
      {/each}
    </select>
  </label>
  {#each accounts as account (account.id)}
    <section class="history-account">
      <h3>{account.name}</h3>
      {#if !account.hasPlotted}
        <p class="empty">No readings in this range.</p>
      {:else}
        {#if account.windowLines.some((l) => l.coords)}
          <div class="history-chart">
            <div class="chart-plot">
              <div class="chart-y" aria-hidden="true">
                {#each PCT_TICKS as t (t.pct)}
                  <span>{t.label}</span>
                {/each}
              </div>
              <svg
                viewBox="0 0 100 {H}"
                preserveAspectRatio="none"
                role="img"
                aria-label="{account.name} usage, percentages 0 to 100, {account.xLabels[0]} to {account.xLabels[2]}"
              >
                {#each PCT_TICKS as t (t.pct)}
                  <line class="chart-grid" x1="0" y1={t.y} x2="100" y2={t.y} vector-effect="non-scaling-stroke" />
                {/each}
                {#each account.windowLines.filter((l) => l.coords) as line (line.id)}
                  <polyline class="usage-line" data-metric={line.id} points={line.coords} stroke={line.color} vector-effect="non-scaling-stroke" />
                {/each}
              </svg>
            </div>
            <div class="chart-x" aria-hidden="true">
              <span>{account.xLabels[0]}</span>
              <span>{account.xLabels[1]}</span>
              <span>{account.xLabels[2]}</span>
            </div>
          </div>
          <!-- The keyed [[chart legend]]: swatch + label + latest value per
               plotted line, so colour is never the sole identifier. Keyed by
               metric id like the lines above. -->
          <p class="history-legend">
            {#each account.windowLines.filter((l) => l.coords) as line (line.id)}
              <span class="legend-item" data-metric={line.id}>
                <span class="legend-swatch" style="background:{line.color}" aria-hidden="true"></span>
                <span class="legend-label">{line.label}</span>
                <span class="legend-value">{line.value}</span>
              </span>
            {/each}
          </p>
        {/if}
        {#if account.credits}
          <div class="credits-chart">
            <div class="chart-plot">
              <div class="chart-y" aria-hidden="true">
                {#each [...account.credits.axis.ticks].reverse() as t (t.y)}
                  <span>{t.label}</span>
                {/each}
              </div>
              <svg
                viewBox="0 0 100 {H}"
                preserveAspectRatio="none"
                role="img"
                aria-label="{account.name} credits balance, {account.credits.axis.lo} to {account.credits.axis.hi}{account.credits.unit ? ` ${account.credits.unit}` : ''}, {account.xLabels[0]} to {account.xLabels[2]}"
              >
                {#each account.credits.axis.ticks as t (t.y)}
                  <line class="chart-grid" x1="0" y1={t.y} x2="100" y2={t.y} vector-effect="non-scaling-stroke" />
                {/each}
                <polyline class="credits-line" points={account.credits.coords} stroke={account.credits.color} vector-effect="non-scaling-stroke" />
              </svg>
            </div>
            <div class="chart-x" aria-hidden="true">
              <span>{account.xLabels[0]}</span>
              <span>{account.xLabels[1]}</span>
              <span>{account.xLabels[2]}</span>
            </div>
          </div>
          <p class="history-legend">
            <span class="legend-item">
              <span class="legend-swatch" style="background:{account.credits.color}" aria-hidden="true"></span>
              <span class="legend-label">{account.credits.unit || 'Credits'}</span>
              <span class="legend-value">{account.credits.value}</span>
            </span>
          </p>
        {/if}
      {/if}
    </section>
  {/each}
</div>
