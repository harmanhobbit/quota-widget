<script>
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';

  const APP_VERSION = __QUOTA_WIDGET_VERSION__;
  let snapshots = $state([]);
  let showBars = $state(true);
  let pinned = $state(false);

  onMount(async () => {
    snapshots = await invoke('get_snapshots');
    const config = await invoke('load_config');
    showBars = config.mini_summary_bars;
    const unlisten = [];
    listen('snapshots', (e) => (snapshots = e.payload)).then((u) => unlisten.push(u));
    listen('config', (e) => (showBars = e.payload.mini_summary_bars)).then((u) => unlisten.push(u));
    return () => unlisten.forEach((u) => u());
  });

  function levelOf(pct) {
    if (pct >= 95) return 'critical';
    if (pct >= 80) return 'warn';
    return 'ok';
  }

  function summarize(snap) {
    if (snap.error) return { text: 'unavailable', level: 'stale' };
    const gating = snap.windows.filter((w) => !w.informational);
    if (gating.length > 0) {
      const worst = gating.reduce((a, b) => (b.used_pct > a.used_pct ? b : a));
      return { text: `${worst.label} ${worst.used_pct.toFixed(0)}%`, level: levelOf(worst.used_pct), pct: Math.min(worst.used_pct, 100) };
    }
    if (snap.credits) return { text: `${snap.credits.balance.toFixed(2)} ${snap.credits.unit}`, level: 'ok' };
    return { text: 'no data', level: 'stale' };
  }

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
  {#if snapshots.length === 0}
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
