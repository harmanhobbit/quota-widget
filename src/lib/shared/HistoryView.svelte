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
  // time would invent a movement that never happened; the note under the
  // chart reports them instead. The host's readings surface (or this note) is
  // where failures live, not the line.
  //
  // All computation below is pure — the template reads one `$derived`, and no
  // helper writes state, so none of it can trip Svelte 5's
  // state_unsafe_mutation.
  //
  // [[usage history]]: ../../CONTEXT.md
  // [[usage window]]: ../../CONTEXT.md
  // [[outcome parity]]: ../../CONTEXT.md
  // [[stale reading]]: ../../CONTEXT.md
  let { history = [], snapshots = [] } = $props();

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

  function timeToX(at, t0, t1) {
    const t = new Date(at).getTime();
    if (t1 <= t0) return 0;
    return Math.min(100, Math.max(0, ((t - t0) / (t1 - t0)) * 100));
  }

  function pctToY(pct) {
    const clamped = Math.min(100, Math.max(0, pct));
    return H - (clamped / 100) * H;
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

  // The credits polyline, auto-scaled to the plotted balances with a small
  // margin; a flat balance draws as a mid-height line rather than dividing
  // by zero.
  function creditCoords(points, t0, t1) {
    const values = points.map((p) => p.credits_balance);
    const lo = Math.min(...values);
    const hi = Math.max(...values);
    const span = hi - lo;
    return points
      .map((p) => {
        const y = span === 0 ? H / 2 : 2 + (1 - (p.credits_balance - lo) / span) * (H - 4);
        return `${timeToX(p.at, t0, t1).toFixed(2)},${y.toFixed(2)}`;
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
      const windowLines = metricIds.map((id) => ({
        id,
        label: snap?.windows?.find((w) => w.metric_id === id)?.label ?? id,
        coords: lineCoords(plotted, id, t0, t1),
      }));
      const creditPoints = plotted.filter((p) => !p.failed && p.credits_balance != null);
      const credits = creditPoints.length
        ? {
            unit: snap?.credits?.unit ?? '',
            coords: creditCoords(creditPoints, t0, t1),
          }
        : null;
      const failedInRange = plotted.filter((p) => p.failed).length;
      const hasPlotted = windowLines.some((l) => l.coords) || credits !== null;
      return {
        id: account.provider_id,
        name: snap?.provider_name ?? account.provider_id,
        windowLines,
        credits,
        failedInRange,
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
            <svg viewBox="0 0 100 {H}" preserveAspectRatio="none" role="img" aria-label="{account.name} usage">
              {#each account.windowLines.filter((l) => l.coords) as line (line.id)}
                <polyline class="usage-line" data-metric={line.id} points={line.coords} vector-effect="non-scaling-stroke" />
              {/each}
            </svg>
          </div>
          <p class="history-legend">
            {#each account.windowLines.filter((l) => l.coords) as line (line.id)}
              <span class="legend-item">{line.label}</span>
            {/each}
          </p>
        {/if}
        {#if account.credits}
          <div class="credits-chart">
            <svg viewBox="0 0 100 {H}" preserveAspectRatio="none" role="img" aria-label="{account.name} credits balance">
              <polyline class="credits-line" points={account.credits.coords} vector-effect="non-scaling-stroke" />
            </svg>
          </div>
          <p class="history-legend">
            <span class="legend-item">{account.credits.unit || 'Credits'}</span>
          </p>
        {/if}
        {#if account.failedInRange > 0}
          <p class="history-note">
            {account.failedInRange} unavailable reading{account.failedInRange === 1 ? '' : 's'} not plotted.
          </p>
        {/if}
      {/if}
    </section>
  {/each}
</div>
