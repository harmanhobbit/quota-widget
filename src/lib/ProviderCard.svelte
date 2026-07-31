<script>
  import { onMount } from 'svelte';

  let { snap } = $props();

  // Ticks once a minute so "resets in …" countdowns stay fresh between polls.
  let now = $state(Date.now());
  onMount(() => {
    const t = setInterval(() => (now = Date.now()), 60_000);
    return () => clearInterval(t);
  });

  function resetsIn(iso) {
    if (!iso) return '';
    const ms = new Date(iso).getTime() - now;
    if (ms <= 0) return 'resets soon';
    const mins = Math.round(ms / 60_000);
    if (mins < 60) return `resets in ${mins}m`;
    const h = Math.floor(mins / 60);
    if (h < 48) return `resets in ${h}h ${mins % 60}m`;
    return `resets in ${Math.floor(h / 24)}d ${h % 24}h`;
  }

  function barClass(pct) {
    if (pct >= 95) return 'critical';
    if (pct >= 80) return 'warn';
    return 'ok';
  }

  function fmtTokens(n) {
    if (n >= 1e9) return (n / 1e9).toFixed(1) + 'B';
    if (n >= 1e6) return (n / 1e6).toFixed(1) + 'M';
    if (n >= 1e3) return (n / 1e3).toFixed(0) + 'k';
    return n.toFixed(0);
  }

  const age = $derived(Math.round((now - new Date(snap.fetched_at).getTime()) / 60_000));
</script>

<div class="card" class:errored={snap.error}>
  <div class="card-head">
    <span class="name">{snap.provider_name}</span>
    <span class="age">{age <= 0 ? 'just now' : `${age}m ago`}</span>
  </div>

  {#if snap.error}
    <p class="error">
      {#if snap.error.kind === 'NotConfigured'}⚪ {snap.error.detail}
      {:else if snap.error.kind === 'AuthExpired'}🔑 {snap.error.detail}
      {:else}⚠ {snap.error.detail}{/if}
    </p>
  {/if}
  {#if !snap.error || snap.windows.length > 0 || snap.credits}
    {#each snap.windows as w (w.label)}
      <div class="window">
        <div class="window-row">
          <span>{w.label}</span>
          <span class="pct">{w.used_pct.toFixed(0)}% · {resetsIn(w.resets_at)}</span>
        </div>
        <div class="bar">
          <div class="fill {barClass(w.used_pct)}" style="width: {Math.min(w.used_pct, 100)}%"></div>
        </div>
      </div>
    {/each}
    {#if snap.credits}
      <div class="credits">
        <span class="balance">{snap.credits.balance.toFixed(2)} {snap.credits.unit}</span>
        {#if snap.credits.used != null}<span class="sub">({snap.credits.used.toFixed(2)} used)</span>{/if}
        {#if snap.credits.est_tokens_remaining != null}
          <span class="sub">≈ {fmtTokens(snap.credits.est_tokens_remaining)} tokens left</span>
        {/if}
      </div>
    {/if}
  {/if}
</div>
