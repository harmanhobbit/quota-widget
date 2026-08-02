<script>
  import { onMount, tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';

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
    }).then((u) => unlisten.push(u));
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

  function levelOf(pct) {
    if (pct >= 95) return 'critical';
    if (pct >= 80) return 'warn';
    return 'ok';
  }

  function summarize(snap) {
    const selected = config?.providers?.[snap.provider_id]?.mini_summary_metric;
    if (selected === 'none') return null;
    if (snap.error) return { text: 'unavailable', level: 'stale' };
    if (selected === 'credits' && snap.credits) return creditSummary(snap.credits);
    if (selected?.startsWith('window:')) {
      const metricId = selected.slice('window:'.length);
      const window = snap.windows.find((candidate) => candidate.metric_id === metricId);
      // A selected informational allowance is still a valid headline; it
      // never changes the tray's separate status and alert calculations.
      if (window) return windowSummary(window);
    }
    const gating = snap.windows.filter((w) => !w.informational);
    if (gating.length > 0) {
      const worst = gating.reduce((a, b) => (b.used_pct > a.used_pct ? b : a));
      return windowSummary(worst);
    }
    if (snap.credits) return creditSummary(snap.credits);
    return { text: 'no data', level: 'stale' };
  }

  const CURRENCY_SYMBOLS = { USD: '$', EUR: '€', GBP: '£', JPY: '¥' };

  // The number is its own right-aligned column so a "0%" lines up under a
  // "100%" and every label after it still starts in the same place.
  const windowSummary = (window) => ({
    value: `${window.used_pct.toFixed(0)}%`,
    label: window.label,
    level: levelOf(window.used_pct),
    pct: Math.min(window.used_pct, 100),
  });
  // The currency is the row's label, matching "5-hour" on a window row, so the
  // amount only needs the bar column — dead space on a credit row — plus the
  // number column, where right-aligning lands it under the percentages.
  const creditSummary = (credits) => ({
    amount: `${CURRENCY_SYMBOLS[credits.unit] ?? ''}${credits.balance.toFixed(2)}`,
    label: credits.unit,
    level: 'ok',
  });

  async function togglePin() {
    pinned = !pinned;
    await invoke('set_mini_pinned', { pinned });
  }
</script>

<div class="mini" bind:this={miniEl}>
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
        {@const s = summarize(snap)}
        {#if s}
          <span class="hover-name">{snap.provider_name}</span>
          {#if s.amount != null}
            <!-- The bar column is dead space on a credit row, so the amount
                 spans it and the number column, ending flush with the
                 percentages; the currency sits in the label column. -->
            <span class="hover-amount {s.level}">{s.amount}</span>
            <span class="hover-label {s.level}">{s.label}</span>
          {:else}
            <!-- Always rendered so a row without a bar (an error) still holds
                 the column open and keeps the numbers aligned. -->
            <span class="hover-bar" class:empty={!(showBars && s.pct != null)}>
              {#if showBars && s.pct != null}
                <i class="fill {s.level}" style="width: {s.pct}%"></i>
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
        {/if}
      {/each}
    </div>
  {/if}
</div>
