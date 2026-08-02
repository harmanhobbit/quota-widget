<script>
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';

  const APP_VERSION = __QUOTA_WIDGET_VERSION__;
  let snapshots = $state([]);
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
    return () => unlisten.forEach((u) => u());
  });

  function levelOf(pct) {
    if (pct >= 95) return 'critical';
    if (pct >= 80) return 'warn';
    return 'ok';
  }

  function summarize(snap) {
    if (snap.error) return { text: 'unavailable', level: 'stale' };
    const selected = config?.providers?.[snap.provider_id]?.mini_summary_metric;
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

  const windowSummary = (window) => ({ text: `${window.label} ${window.used_pct.toFixed(0)}%`, level: levelOf(window.used_pct), pct: Math.min(window.used_pct, 100) });
  const creditSummary = (credits) => ({ text: `${credits.balance.toFixed(2)} ${credits.unit}`, level: 'ok' });

  async function togglePin() {
    pinned = !pinned;
    await invoke('set_mini_pinned', { pinned });
  }
</script>

<div class="mini">
  <header data-tauri-drag-region>
    <span data-tauri-drag-region>Quota Widget <small class="build-version" data-tauri-drag-region>v{APP_VERSION}</small></span>
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
    {#each snapshots as snap (snap.provider_id)}
      {@const s = summarize(snap)}
      <div class="hover-row">
        <span class="hover-name">{snap.provider_name}</span>
        {#if showBars && s.pct != null}
          <span class="hover-bar"><i class="fill {s.level}" style="width: {s.pct}%"></i></span>
        {/if}
        <span class="hover-val {s.level}">{s.text}</span>
      </div>
    {/each}
  {/if}
</div>
