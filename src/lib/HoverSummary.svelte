<script>
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';

  let snapshots = $state([]);

  onMount(() => {
    invoke('get_snapshots').then((s) => (snapshots = s));
    const unlisten = [];
    listen('snapshots', (e) => (snapshots = e.payload)).then((u) => unlisten.push(u));
    return () => unlisten.forEach((u) => u());
  });

  function levelOf(pct) {
    if (pct >= 95) return 'critical';
    if (pct >= 80) return 'warn';
    return 'ok';
  }

  // One line per provider: its worst real window, else its balance. Windows
  // flagged informational don't gate anything, so they never headline.
  function summarize(snap) {
    if (snap.error) return { text: 'unavailable', level: 'stale' };
    const gating = snap.windows.filter((w) => !w.informational);
    if (gating.length > 0) {
      const worst = gating.reduce((a, b) => (b.used_pct > a.used_pct ? b : a));
      return {
        text: `${worst.label} ${worst.used_pct.toFixed(0)}%`,
        level: levelOf(worst.used_pct),
        pct: Math.min(worst.used_pct, 100),
      };
    }
    if (snap.credits) {
      return { text: `${snap.credits.balance.toFixed(2)} ${snap.credits.unit}`, level: 'ok' };
    }
    return { text: 'no data', level: 'stale' };
  }
</script>

<div class="hover">
  {#if snapshots.length === 0}
    <p class="hover-empty">No providers enabled</p>
  {:else}
    {#each snapshots as snap (snap.provider_id)}
      {@const s = summarize(snap)}
      <div class="hover-row">
        <span class="hover-name">{snap.provider_name}</span>
        {#if s.pct != null}
          <span class="hover-bar"><i class="fill {s.level}" style="width: {s.pct}%"></i></span>
        {/if}
        <span class="hover-val {s.level}">{s.text}</span>
      </div>
    {/each}
  {/if}
</div>
