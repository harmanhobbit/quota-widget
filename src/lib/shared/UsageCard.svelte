<script>
  import { onMount } from 'svelte';
  import { resetsIn as fmtResetsIn, periodProgress as fmtPeriodProgress } from '../period.js';

  // `schedule` is the account's `usage_schedule` (serialized quota-core
  // `UsageSchedule`), looked up from config by the parent that owns this card.
  // Omitted by callers that predate the schedule (e.g. a snapshot with no
  // matching config entry), which `period.js` treats as the raw calendar marker
  // — so a card with no schedule renders exactly as it always did.
  let { snap, schedule } = $props();

  // Ticks once a minute so "resets in …" countdowns stay fresh between polls.
  let now = $state(Date.now());
  onMount(() => {
    const t = setInterval(() => (now = Date.now()), 60_000);
    return () => clearInterval(t);
  });

  // The window label whose bar is pressed and held, or null when none is. A
  // momentary peek, never persisted: holding a bar shows the calendar marker —
  // where the marker would sit with no schedule applied — and releasing reverts
  // to the scheduled marker. Omitted here for the peek, `schedule` is treated
  // by `period.js` as the raw calendar fraction, so the two markers coincide on
  // a non-weekly window or an all-seven schedule.
  let peek = $state(null);

  const resetsIn = (iso) => fmtResetsIn(iso, now);
  const periodProgress = (w) => fmtPeriodProgress(w, now, peek === w.label ? undefined : schedule);

  function releasePeek() {
    peek = null;
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
      {:else if snap.error.kind === 'Unavailable'}🔒 {snap.error.detail}
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
        <!-- Deliberately pointer-only and out of the tab order: the peek is a
             redundant enhancement, and the marker it moves is aria-hidden, so
             there is no semantics a keyboard user would be missing. -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="bar"
          onpointerdown={() => (peek = w.label)}
          onpointerup={releasePeek}
          onpointercancel={releasePeek}
          onpointerleave={releasePeek}
          oncontextmenu={(e) => e.preventDefault()}
        >
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
