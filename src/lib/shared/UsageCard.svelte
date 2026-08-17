<script>
  import { onMount } from 'svelte';
  import { resetsIn as fmtResetsIn, periodProgress as fmtPeriodProgress } from '../period.js';

  let { snap } = $props();

  // Ticks once a minute so "resets in …" countdowns stay fresh between polls.
  let now = $state(Date.now());
  onMount(() => {
    const t = setInterval(() => (now = Date.now()), 60_000);
    return () => clearInterval(t);
  });

  const resetsIn = (iso) => fmtResetsIn(iso, now);
  const periodProgress = (w) => fmtPeriodProgress(w, now);

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

  function fmtAmount(n) {
    return n.toLocaleString(undefined, { maximumFractionDigits: 2 });
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
            style="width: {Math.max(0, Math.min(w.used_pct, 100))}%"
          ></div>
          <!-- Decorative: the "resets in …" text already states the time left,
               so this is a second reading of it, not new information. -->
          {#if progress != null}
            <i class="period-mark" style="left: {progress * 100}%" aria-hidden="true"></i>
          {/if}
        </div>
        {#if w.allowance}
          <span class="sub allowance">{fmtAmount(w.allowance.remaining)} / {fmtAmount(w.allowance.total)} {w.allowance.unit} remaining</span>
        {/if}
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
