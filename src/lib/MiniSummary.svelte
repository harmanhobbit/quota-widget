<script>
  import { onMount, tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { resetOpacity, stepOpacity } from './opacity.js';
  import { periodProgress as fmtPeriodProgress, periodTooltip } from './period.js';

  const APP_VERSION = __QUOTA_WIDGET_VERSION__;
  const BUILD_BRANCH = __QUOTA_WIDGET_BRANCH__;
  let snapshots = $state([]);
  let miniEl = $state(null);
  let showBars = $state(true);
  let pinned = $state(false);
  let config = $state(null);
  let loadError = $state('');
  // Distinguishes "first load still in flight" from "no providers enabled",
  // so the window never looks blank while it is simply waiting.
  let loaded = $state(false);

  onMount(async () => {
    resetOpacity();
    try {
      const initial = await invoke('get_snapshots');
      snapshots = initial.snapshots;
      config = initial.config;
      showBars = initial.config.mini_summary_bars;
    } catch (error) {
      loadError = `Could not load summary: ${String(error)}`;
    }
    loaded = true;
    const unlisten = [];
    listen('snapshots', (e) => {
      snapshots = e.payload;
      // A later successful push supersedes a stale initial-load failure.
      loadError = '';
    }).then((u) => unlisten.push(u));
    listen('config', (e) => {
      config = e.payload;
      showBars = e.payload.mini_summary_bars;
      // Turning the preference off must restore a window left mid-fade, not
      // freeze it at whatever level it happened to be on.
      if (!e.payload.scroll_opacity) resetOpacity();
    }).then((u) => unlisten.push(u));
    // The webview survives hiding, so the fade level has to be cleared on
    // every show or the summary reopens as invisible as it was left.
    listen('mini-shown', resetOpacity).then((u) => unlisten.push(u));
    // Hiding the window resets the pin in Rust; the webview survives that, so
    // the button has to follow or it reopens looking pinned when it isn't.
    listen('mini-pinned', (e) => (pinned = e.payload)).then((u) => unlisten.push(u));
    return () => unlisten.forEach((u) => u());
  });

  // The window is a fixed height in tauri.conf.json, which leaves dead space
  // under a short account list. Unlike App this cannot call `setSize` itself —
  // the mini capability grants no window-management permissions — so it
  // measures and lets Rust resize and re-anchor in one step.
  async function fitHeight() {
    if (!miniEl) return;
    try {
      await invoke('set_mini_height', { height: miniEl.offsetHeight });
    } catch {
      // sizing is cosmetic — never let it break the UI
    }
  }

  $effect(() => {
    void snapshots;
    void showBars;
    void loadError;
    void loaded;
    tick().then(fitHeight);
  });

  // Ticks once a minute so the period markers creep along between polls, the
  // same cadence ProviderCard uses for its countdowns.
  let now = $state(Date.now());
  onMount(() => {
    const t = setInterval(() => (now = Date.now()), 60_000);
    return () => clearInterval(t);
  });

  const periodProgress = (w) => fmtPeriodProgress(w, now);

  // Horizontal distance from the pointer at which the period marker starts to
  // grow, and the distance within which its tooltip is armed. Pixels, not
  // percent: the bar is only ~60–90px wide here, so a percentage-based zone
  // would be a couple of pixels and unhittable.
  const APPROACH_PX = 24;
  // Deliberately a small fraction of the approach radius: the marker's growth
  // is the cue that the tooltip is coming, so arming too early means the
  // tooltip lands before the growth has been seen.
  const TOOLTIP_PX = 4;

  // Which row's marker the pointer is near, and how near. Only ever one row:
  // proximity is measured inside the hovered bar, so passing over the summary
  // doesn't set every marker animating at once.
  let nearest = $state(null);

  function markerX(bar, progress) {
    const box = bar.getBoundingClientRect();
    return { left: box.left + box.width * progress, box };
  }

  function trackPointer(event, key, progress) {
    // Queried rather than walked from the target: the marker sits between the
    // bar and the target, so sibling order is not a stable way to find it.
    const bar = event.currentTarget.parentElement?.querySelector('.hover-bar');
    if (!bar) return;
    const { left, box } = markerX(bar, progress);
    // Clamped at the ends: a marker at 100% keeps its zone on the bar rather
    // than letting it hang off into the row's padding.
    const x = Math.min(Math.max(event.clientX, box.left), box.right);
    nearest = { key, distance: Math.abs(x - left) };
  }

  // 0 at the approach radius, 1 on the marker. Drives width and opacity
  // directly so the growth tracks the pointer instead of easing behind it.
  function approach(key) {
    if (nearest?.key !== key) return 0;
    return Math.max(0, 1 - nearest.distance / APPROACH_PX);
  }

  const armed = (key) => nearest?.key === key && nearest.distance <= TOOLTIP_PX;

  function levelOf(pct) {
    if (pct >= 95) return 'critical';
    if (pct >= 80) return 'warn';
    return 'ok';
  }

  // One row per selected headline. An empty array omits the account entirely;
  // `null` selection means automatic, which still resolves to a single row.
  function summarize(snap) {
    const selected = config?.providers?.[snap.provider_id]?.mini_summary_metrics;
    if (selected?.length === 0) return [];
    if (snap.error) return [{ text: 'unavailable', level: 'stale' }];
    if (selected) {
      // A selected informational allowance is still a valid headline; it
      // never changes the tray's separate status and alert calculations.
      const rows = selected.map((metric) => metricSummary(snap, metric)).filter(Boolean);
      // Every chosen metric having vanished from the snapshot is worth saying
      // out loud rather than silently dropping the account.
      return rows.length > 0 ? rows : [{ text: 'no data', level: 'stale' }];
    }
    const gating = snap.windows.filter((w) => !w.informational);
    if (gating.length > 0) {
      const worst = gating.reduce((a, b) => (b.used_pct > a.used_pct ? b : a));
      return [windowSummary(worst)];
    }
    if (snap.credits) return [creditSummary(snap.credits)];
    return [{ text: 'no data', level: 'stale' }];
  }

  function metricSummary(snap, metric) {
    if (metric === 'credits') return snap.credits ? creditSummary(snap.credits) : null;
    if (!metric.startsWith('window:')) return null;
    const metricId = metric.slice('window:'.length);
    const window = snap.windows.find((candidate) => candidate.metric_id === metricId);
    return window ? windowSummary(window) : null;
  }

  const CURRENCY_SYMBOLS = { USD: '$', EUR: '€', GBP: '£', JPY: '¥' };

  // The number is its own right-aligned column so a "0%" lines up under a
  // "100%" and every label after it still starts in the same place.
  const windowSummary = (window) => ({
    value: `${window.used_pct.toFixed(0)}%`,
    label: window.label,
    level: levelOf(window.used_pct),
    pct: Math.min(window.used_pct, 100),
    progress: periodProgress(window),
    // Kept so the period marker's tooltip can read the reset time. The row
    // itself shows no countdown, which is what makes that tooltip worth having.
    window,
  });
  // The currency is the row's label, matching "5-hour" on a window row. The bar
  // column is dead space on a credit row, so the amount sits centred in it and
  // the currency takes the number and label columns after it. A spend figure
  // carries its own label ("Cost this month") and uses that instead, so the
  // compact row never presents money spent as money remaining.
  const creditSummary = (credits) => ({
    amount: `${CURRENCY_SYMBOLS[credits.unit] ?? ''}${credits.balance.toFixed(2)}`,
    label: credits.label ?? credits.unit,
    level: 'ok',
  });

  // The summary has no scrollable content of its own, so unlike the popup
  // every wheel event over it is a fade. How far it may fade depends on the
  // pin: an unpinned window is dismissed by the next click anywhere else, so
  // fully transparent is recoverable. A pinned one is always-on-top and
  // ignores click-away, so at zero it would be an invisible thing eating
  // clicks with no way back — floor it where it stays findable.
  function fadeOnWheel(event) {
    if (!config?.scroll_opacity) return;
    if (stepOpacity(event.deltaY, pinned ? 0.15 : 0)) event.preventDefault();
  }

  async function togglePin() {
    pinned = !pinned;
    await invoke('set_mini_pinned', { pinned });
  }
</script>

<div class="mini" bind:this={miniEl} onwheel={fadeOnWheel}>
  <header data-tauri-drag-region>
    <span data-tauri-drag-region>Quota Widget <small class="build-version" data-tauri-drag-region>v{APP_VERSION}</small>{#if BUILD_BRANCH} <small class="build-branch" data-tauri-drag-region>{BUILD_BRANCH}</small>{/if}</span>
    <span class="spacer" data-tauri-drag-region></span>
    <button class="icon mini-pin" title={pinned ? 'Unpin summary' : 'Pin summary'} onclick={togglePin}>{pinned ? '●' : '○'}</button>
    <button class="icon mini-close" title="Hide summary" onclick={() => invoke('hide_window')}>✕</button>
  </header>
  {#if loadError}
    <p class="hover-empty">{loadError}</p>
  {:else if !loaded}
    <p class="hover-empty">Loading…</p>
  {:else if snapshots.length === 0}
    <p class="hover-empty">No providers enabled</p>
  {:else}
    <!-- One shared grid, not per-row flex: the bar and value columns must line
         up across accounts whose names differ in width. -->
    <div class="hover-rows">
      {#each snapshots as snap (snap.provider_id)}
        {#each summarize(snap) as s, row (row)}
          <!-- The name labels the group, so later rows of the same account
               leave the cell empty rather than repeating it. -->
          <span class="hover-name">{row === 0 ? snap.provider_name : ''}</span>
          {#if s.amount != null}
            <!-- The bar column is dead space on a credit row, so the amount is
                 centred in it and the currency spans the number and label
                 columns that follow. -->
            <span class="hover-amount {s.level}">{s.amount}</span>
            <span class="hover-label span {s.level}">{s.label}</span>
          {:else}
            <!-- Always rendered so a row without a bar (an error) still holds
                 the column open and keeps the numbers aligned. -->
            <!-- The bar clips its overflow to keep the fill inside its rounded
                 ends, and hit-testing follows that clip — so the hover target
                 is a sibling laid over the bar, spanning the full row height
                 rather than the bar's 5px. -->
            {@const key = `${snap.provider_id}:${row}`}
            <span class="hover-bar-cell">
              <span class="hover-bar" class:empty={!(showBars && s.pct != null)}>
                {#if showBars && s.pct != null}
                  <i class="fill {s.level}" style="width: {s.pct}%"></i>
                {/if}
              </span>
              <!-- How far through the period we are, so usage can be read
                   against time left. Skipped when the provider can't bound the
                   period rather than guessed at. It sits on the cell rather
                   than inside the bar so it can grow taller than the bar's
                   5px, and grows towards the pointer so its tooltip is
                   something you aim at rather than fall into. The cell lays
                   the bar out to fill its width, so the same percentage puts
                   the marker in the same place either way. -->
              {#if showBars && s.pct != null && s.progress != null}
                <i
                  class="period-mark"
                  style="left: {s.progress * 100}%; height: calc(var(--hover-bar-h) + {approach(key) * 6}px); opacity: {0.8 + approach(key) * 0.2}"
                  aria-hidden="true"
                ></i>
                <!-- Deliberately pointer-only and out of the tab order. This
                     window is a tray popup that hides on focus loss, and the
                     same countdown is visible text on the full card, so the
                     tooltip enhances a redundant surface rather than being the
                     sole channel for the reset time. `data-tip` rather than
                     `title`: the OS draws `title` after a delay of its own,
                     which is too slow to scrub against. -->
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <span
                  class="hover-bar-target"
                  onpointermove={(e) => trackPointer(e, key, s.progress)}
                  onpointerleave={() => (nearest = null)}
                  data-tip={armed(key) ? periodTooltip(s.window, s.progress, now) : null}
                ></span>
              {/if}
            </span>
            {#if s.value != null}
              <span class="hover-val {s.level}">{s.value}</span>
              <span class="hover-label {s.level}">{s.label}</span>
            {:else}
              <!-- Status text is one phrase, so it spans number and label. -->
              <span class="hover-val span {s.level}">{s.text}</span>
            {/if}
          {/if}
        {/each}
      {/each}
    </div>
  {/if}
</div>
