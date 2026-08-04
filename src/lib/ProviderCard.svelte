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

  // How far through the window's period we are, 0–1, or null when the provider
  // couldn't tell us the period's bounds. Drawn against the usage bar so a
  // half-full bar at the quarter mark reads as "burning it fast".
  function periodProgress(w) {
    if (!w.resets_at || !w.period_start) return null;
    const start = new Date(w.period_start).getTime();
    const end = new Date(w.resets_at).getTime();
    const span = end - start;
    if (!(span > 0)) return null;
    return Math.min(Math.max((now - start) / span, 0), 1);
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
      {@const progress = periodProgress(w)}
      <div class="window" class:informational={w.informational}>
        <div class="window-row">
          <span>{w.label}</span>
          <span class="pct">{w.used_pct.toFixed(0)}% · {resetsIn(w.resets_at)}</span>
        </div>
        <div class="bar">
          <!-- Informational windows never colour by threshold: they don't
               gate anything, so red would be misleading. -->
          <div
            class="fill {w.informational ? 'muted' : barClass(w.used_pct)}"
            style="width: {Math.min(w.used_pct, 100)}%"
          ></div>
          <!-- Decorative: the "resets in …" text already states the time left,
               so this is a second reading of it, not new information. -->
          {#if progress != null}
            <i class="period-mark" style="left: {progress * 100}%" aria-hidden="true"></i>
          {/if}
        </div>
      </div>
    {/each}
    {#if snap.credits}
      <div class="credits">
        <!-- A labelled amount is spend, not a balance ("Cost this month: 12.30
             USD"); an unlabelled one is money left and reads as a bare figure. -->
        <span class="balance">{snap.credits.label ? `${snap.credits.label}: ` : ''}{snap.credits.balance.toFixed(2)} {snap.credits.unit}</span>
        {#if snap.credits.used != null}<span class="sub">({snap.credits.used.toFixed(2)} used)</span>{/if}
        {#if snap.credits.est_tokens_remaining != null}
          <span class="sub">≈ {fmtTokens(snap.credits.est_tokens_remaining)} tokens left</span>
        {/if}
      </div>
    {/if}
  {/if}
</div>
