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
  // state_unsafe_mutation. The only state writes are the event handlers at
  // the bottom of the script, which is where they belong.
  //
  // Line colours: one source of truth per series, assigned in buildAccounts
  // and applied to BOTH the polyline stroke and its legend swatch, so the two
  // cannot disagree. (Colouring used to live in CSS as .usage-line:nth-of-type(n)
  // — keyed on rendered position, so a line dropping out of the range silently
  // recoloured its neighbours.) Assignment is by index in the account's
  // metric-id list, which is computed over ALL recorded points and is therefore
  // range-independent: a line keeps its colour as the range selector moves.
  // The palette holds hues measured to at least 3:1 contrast against BOTH
  // theme backgrounds (#f5f5f7 light, #1e1e22 dark — WCAG 1.4.11 for the
  // lines, headroom for the 8px swatches); the credits line keeps its own
  // distinct colour, never shared with a window.
  //
  // Scrub: every chart is a scrub surface ([[scrub]]) feeding one [[scrub
  // readout]] box. A pointer position or a keyboard move selects the nearest
  // in-range history-point column — failed points included on the percentage
  // chart, so a failure reads as *unavailable* rather than being skipped —
  // and the readout reports the timestamp plus each series' figure there.
  // Pointer and keyboard write the same per-chart state, so both inputs
  // drive one identical readout.
  //
  // [[usage history]]: ../../CONTEXT.md
  // [[usage window]]: ../../CONTEXT.md
  // [[chart legend]]: ../../CONTEXT.md
  // [[scrub]]: ../../CONTEXT.md
  // [[scrub readout]]: ../../CONTEXT.md
  // [[outcome parity]]: ../../CONTEXT.md
  // [[stale reading]]: ../../CONTEXT.md
  let { history = [], snapshots = [] } = $props();

  const SERIES_PALETTE = ['#277fc5', '#b77900', '#9a6dd7', '#b34d80', '#007f7f', '#238b57'];
  const CREDITS_COLOR = '#147d64';

  // The selectable time ranges. `null` means unbounded ("all time").
  const RANGES = [
    { key: '24h', label: '24 hours', ms: 24 * 3600_000 },
    { key: '7d', label: '7 days', ms: 7 * 24 * 3600_000 },
    { key: '30d', label: '30 days', ms: 30 * 24 * 3600_000 },
    { key: 'all', label: 'All time', ms: null },
  ];
  let range = $state('7d');

  // One [[scrub]] position per chart, as an index into that chart's columns:
  // keyed by `${chart}:${account id}` so the usage and credits charts scrub
  // independently and the keyboard and pointer paths drive the same state.
  let reads = $state({});

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

  // ---- [[scrub readout]] helpers. All pure: they read the selection state,
  // never write it, so the template can call them freely. ----

  // The column a chart's selection points at, or null when there is no
  // selection or the index has gone stale (an out-of-range index reads as no
  // selection rather than a wrong figure or a throw).
  function readColumn(account, kind, reads) {
    const columns = kind === 'credits' ? account.credits?.columns : account.usage?.columns;
    const i = reads[`${kind}:${account.id}`];
    if (i == null || !columns) return null;
    return columns[i] ?? null;
  }

  // The readout names the moment as a date AND a time: scrubbing answers
  // "what was this here", and a bare date cannot separate two readings hours
  // apart.
  function readoutWhen(at) {
    const d = new Date(at);
    return `${d.toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}, ${d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })}`;
  }

  // Per-series figures at the selected percentage point: every plotted line
  // with its value there, null marking a window absent at that point (the
  // template renders the dash). A failed point carries no figures at all —
  // the readout says so rather than inventing one.
  function usageReadLines(account, column) {
    if (!column || column.failed) return [];
    return account.windowLines
      .filter((l) => l.coords)
      .map((l) => ({
        color: l.color,
        label: l.label,
        value: column.windows.find(([id]) => id === l.id)?.[1] ?? null,
      }));
  }

  // Guide dots at the selected percentage point: one per plotted line that
  // has a figure at that exact point, none on a failed point — a failure
  // carries no figure to mark. The dot reuses the line's colour (the same
  // one-source value as the stroke and legend swatch).
  function usageDots(account, column) {
    if (!column || column.failed) return [];
    return account.windowLines
      .filter((l) => l.coords)
      .flatMap((l) => {
        const w = column.windows.find(([id]) => id === l.id);
        return w ? [{ color: l.color, x: column.tx, y: pctToY(w[1]) }] : [];
      });
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
      // The [[scrub]] columns. The percentage chart's columns are ALL points
      // in range — a failed reading is a real recorded moment, and selecting
      // it is how its *unavailable* gets explained. The credits chart's
      // columns are its plotted set only (non-failed balances). Positions are
      // precomputed in the same viewBox units the lines use, so pointer
      // nearest-column matching and the guide/dots share one mapping.
      const usage = {
        columns: plotted.map((p) => ({
          at: p.at,
          tx: timeToX(p.at, t0, t1),
          failed: !!p.failed,
          windows: p.windows ?? [],
        })),
      };
      let credits = null;
      if (creditPoints.length) {
        const values = creditPoints.map((p) => p.credits_balance);
        const lo = values.reduce((a, b) => (b < a ? b : a));
        const hi = values.reduce((a, b) => (b > a ? b : a));
        const axis = niceAxis(lo, hi);
        const unit = snap?.credits?.unit ?? '';
        const latest = creditPoints[creditPoints.length - 1].credits_balance;
        const span = axis.hi - axis.lo;
        credits = {
          unit,
          color: CREDITS_COLOR,
          axis,
          coords: creditCoords(creditPoints, axis, t0, t1),
          columns: creditPoints.map((p) => ({
            at: p.at,
            tx: timeToX(p.at, t0, t1),
            balance: p.credits_balance,
            y: Math.min(H, Math.max(0, H - ((p.credits_balance - axis.lo) / span) * H)),
          })),
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
        usage,
        credits,
        // The plotted span's time labels: start, middle, end — the X axis
        // every chart in this account shares.
        xLabels: [t0, (t0 + t1) / 2, t1].map((t) => rangeLabel(t, rangeKey)),
        hasPlotted,
      };
    });
  }

  let accounts = $derived(buildAccounts(history, snapshots, range));

  // ---- [[scrub]] event handlers. The only state writers in the file, and
  // the keyboard and pointer paths converge on the same `reads` state, so
  // both inputs drive one identical readout per chart. ----

  function scrubPointer(event, account, kind) {
    const columns = kind === 'credits' ? account.credits?.columns : account.usage?.columns;
    if (!columns?.length) return;
    // Zero-width geometry (a hidden pane, or jsdom's no-layout mounts) has no
    // width to divide by: the pointer is clamped into the box first — the
    // same read-it-as-on-the-target trick the card tooltip uses — and a
    // zero-width plot reads every position as its left edge, a valid column.
    const box = event.currentTarget.getBoundingClientRect();
    const width = box.right - box.left;
    const x = Math.min(Math.max(event.clientX, box.left), box.right);
    const frac = width > 0 ? (x - box.left) / width : 0;
    // Nearest column by normalised time: positions are precomputed in the
    // same 0–100 space the lines draw in, so what the pointer picks is what
    // the guide later marks.
    let best = 0;
    let bestDist = Infinity;
    columns.forEach((column, i) => {
      const dist = Math.abs(column.tx / 100 - frac);
      if (dist < bestDist) {
        bestDist = dist;
        best = i;
      }
    });
    reads[`${kind}:${account.id}`] = best;
  }

  // Leaving the plot disarms the readout; the box itself stays so the fade
  // has something to animate, exactly like the card tooltip's armed box.
  function clearRead(accountId, kind) {
    reads[`${kind}:${accountId}`] = null;
  }

  // Keyboard scrub (desktop route): no selection yet and the arrows pick the
  // sensible end — Right from nothing starts at the oldest point, Left at the
  // newest — then move one column, clamped. Home/End jump. Every handled key
  // preventDefaults so the pane under the focused chart does not scroll.
  function scrubKey(event, account, kind) {
    const columns = kind === 'credits' ? account.credits?.columns : account.usage?.columns;
    if (!columns?.length) return;
    const key = `${kind}:${account.id}`;
    const current = reads[key];
    const last = columns.length - 1;
    let next = null;
    if (event.key === 'ArrowRight') next = current == null ? 0 : Math.min(last, current + 1);
    else if (event.key === 'ArrowLeft') next = current == null ? last : Math.max(0, current - 1);
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = last;
    else return;
    event.preventDefault();
    reads[key] = next;
  }
</script>

<div class="history-view">
  <label class="history-range-label">Range
    <!-- A range change invalidates every selection: the columns it indexed
         into no longer exist, and a stale figure is worse than no figure. -->
    <select class="history-range" bind:value={range} onchange={() => { reads = {}; }}>
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
              <!-- The [[scrub]] surface: pointer/press on every platform,
                   keyboard on desktop. Focusable so arrow keys reach it; the
                   dots overlay is pointer-transparent so press-drag always
                   hits the svg. -->
              <div class="chart-svg">
                <!-- A focusable graphic: role="img" keeps the chart's
                     description for screen readers while tabindex makes it
                     the desktop keyboard scrub surface, and the values reach
                     assistive tech through the readout's aria-live. The a11y
                     lint has no shape for an interactive image, so this one
                     suppression is deliberate. -->
                <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
                <svg
                  viewBox="0 0 100 {H}"
                  preserveAspectRatio="none"
                  role="img"
                  aria-label="{account.name} usage, percentages 0 to 100, {account.xLabels[0]} to {account.xLabels[2]}"
                  tabindex="0"
                  onpointerdown={(e) => scrubPointer(e, account, 'usage')}
                  onpointermove={(e) => scrubPointer(e, account, 'usage')}
                  onpointerleave={() => clearRead(account.id, 'usage')}
                  onkeydown={(e) => scrubKey(e, account, 'usage')}
                >
                  {#each PCT_TICKS as t (t.pct)}
                    <line class="chart-grid" x1="0" y1={t.y} x2="100" y2={t.y} vector-effect="non-scaling-stroke" />
                  {/each}
                  {#each account.windowLines.filter((l) => l.coords) as line (line.id)}
                    <polyline class="usage-line" data-metric={line.id} points={line.coords} stroke={line.color} vector-effect="non-scaling-stroke" />
                  {/each}
                  {#if readColumn(account, 'usage', reads)}
                    {@const column = readColumn(account, 'usage', reads)}
                    <!-- The guide marks the selected moment — except on a
                         failed point, where there is no figure to point at. -->
                    {#if !column.failed}
                      <line class="chart-guide" x1={column.tx} y1="0" x2={column.tx} y2={H} vector-effect="non-scaling-stroke" />
                    {/if}
                  {/if}
                </svg>
                <!-- Dots are HTML, not SVG circles: the viewBox stretches
                     non-uniformly, which would pull every circle into an
                     ellipse. Same reasoning as the HTML tick labels. -->
                <div class="chart-dots" aria-hidden="true">
                  {#each usageDots(account, readColumn(account, 'usage', reads)) as dot, i (i)}
                    <span class="chart-dot" style="left:{dot.x}%; top:{(dot.y / H) * 100}%; background:{dot.color}"></span>
                  {/each}
                </div>
              </div>
            </div>
            <div class="chart-x" aria-hidden="true">
              <span>{account.xLabels[0]}</span>
              <span>{account.xLabels[1]}</span>
              <span>{account.xLabels[2]}</span>
            </div>
          </div>
          <!-- The [[scrub readout]]: one always-present box per chart, armed
               by either input path, emptied when the selection clears so no
               stale figure survives a range change. -->
          <p class="chart-readout" data-armed={readColumn(account, 'usage', reads) ? '' : null} aria-live="polite">
            {#if readColumn(account, 'usage', reads)}
              {@const column = readColumn(account, 'usage', reads)}
              <span class="readout-when">{readoutWhen(column.at)}</span>
              {#if column.failed}
                <span class="readout-unavailable">unavailable</span>
              {:else}
                {#each usageReadLines(account, column) as line (line.label)}
                  <span class="readout-item">
                    <span class="legend-swatch" style="background:{line.color}" aria-hidden="true"></span>
                    <span class="legend-label">{line.label}</span>
                    <span class="legend-value">{line.value == null ? '—' : `${Math.round(line.value)}%`}</span>
                  </span>
                {/each}
              {/if}
            {/if}
          </p>
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
              <div class="chart-svg">
                <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
                <svg
                  viewBox="0 0 100 {H}"
                  preserveAspectRatio="none"
                  role="img"
                  aria-label="{account.name} credits balance, {account.credits.axis.lo} to {account.credits.axis.hi}{account.credits.unit ? ` ${account.credits.unit}` : ''}, {account.xLabels[0]} to {account.xLabels[2]}"
                  tabindex="0"
                  onpointerdown={(e) => scrubPointer(e, account, 'credits')}
                  onpointermove={(e) => scrubPointer(e, account, 'credits')}
                  onpointerleave={() => clearRead(account.id, 'credits')}
                  onkeydown={(e) => scrubKey(e, account, 'credits')}
                >
                  {#each account.credits.axis.ticks as t (t.y)}
                    <line class="chart-grid" x1="0" y1={t.y} x2="100" y2={t.y} vector-effect="non-scaling-stroke" />
                  {/each}
                  <polyline class="credits-line" points={account.credits.coords} stroke={account.credits.color} vector-effect="non-scaling-stroke" />
                  {#if readColumn(account, 'credits', reads)}
                    <line class="chart-guide" x1={readColumn(account, 'credits', reads).tx} y1="0" x2={readColumn(account, 'credits', reads).tx} y2={H} vector-effect="non-scaling-stroke" />
                  {/if}
                </svg>
                <div class="chart-dots" aria-hidden="true">
                  {#if readColumn(account, 'credits', reads)}
                    <span class="chart-dot" style="left:{readColumn(account, 'credits', reads).tx}%; top:{(readColumn(account, 'credits', reads).y / H) * 100}%; background:{account.credits.color}"></span>
                  {/if}
                </div>
              </div>
            </div>
            <div class="chart-x" aria-hidden="true">
              <span>{account.xLabels[0]}</span>
              <span>{account.xLabels[1]}</span>
              <span>{account.xLabels[2]}</span>
            </div>
          </div>
          <p class="chart-readout" data-armed={readColumn(account, 'credits', reads) ? '' : null} aria-live="polite">
            {#if readColumn(account, 'credits', reads)}
              {@const column = readColumn(account, 'credits', reads)}
              <span class="readout-when">{readoutWhen(column.at)}</span>
              <span class="readout-item">
                <span class="legend-swatch" style="background:{account.credits.color}" aria-hidden="true"></span>
                <span class="legend-value">{Math.round(column.balance * 100) / 100}{account.credits.unit ? ` ${account.credits.unit}` : ''}</span>
              </span>
            {/if}
          </p>
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
