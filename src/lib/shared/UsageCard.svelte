<script>
  import { onMount } from 'svelte';
  import { resetsIn as fmtResetsIn, periodProgress as fmtPeriodProgress, periodTooltip } from '../period.js';

  // `schedule` is the account's `usage_schedule` (serialized quota-core
  // `UsageSchedule`), looked up from config by the parent that owns this card.
  // Omitted by callers that predate the schedule (e.g. a snapshot with no
  // matching config entry), which `period.js` treats as the raw calendar marker
  // — so a card with no schedule renders exactly as it always did.
  //
  // `hover` opts the card into the mini summary's period-marker affordance
  // (proximity growth + armed tooltip). It is deliberately opt-in per surface:
  // the card is shared with the Android foreground app, where a touch's
  // pointermove during the press-and-hold peek would otherwise grow the marker
  // and arm the tooltip, changing the clean peek #124 shipped. The desktop
  // popup passes it; MobileApp omits it, and with it off the card renders
  // exactly as it always did.
  let { snap, schedule, hover = false } = $props();

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

  // ---- period-marker hover (opt-in via `hover`) -----------------------------
  // The mini summary's affordance, mirrored rather than extracted: the same
  // distances and the same shape, so the two surfaces feel identical.

  // Horizontal distance from the pointer at which the period marker starts to
  // grow, and the distance within which its tooltip is armed. The mini's
  // values, kept identical so the affordance reads the same one click deeper
  // into the app.
  const APPROACH_PX = 44;
  const TOOLTIP_PX = 4;

  // Which window's marker the pointer is near, and how near. Only ever one
  // window: proximity is measured inside the hovered bar, so crossing the card
  // doesn't set every marker animating at once. Keyed by window label, the
  // same key the peek uses.
  let nearest = $state(null);

  function markerX(bar, progress) {
    const box = bar.getBoundingClientRect();
    return { left: box.left + box.width * progress, box };
  }

  function trackPointer(event, key, progress) {
    // Queried rather than walked from the target: the marker sits between the
    // bar and the target, so sibling order is not a stable way to find it.
    const bar = event.currentTarget.parentElement?.querySelector('.bar');
    if (!bar) return;
    const { left, box } = markerX(bar, progress);
    // Clamped at the ends: a marker at 100% keeps its zone on the bar rather
    // than letting it hang off into the card's padding.
    const x = Math.min(Math.max(event.clientX, box.left), box.right);
    nearest = { key, distance: Math.abs(x - left) };
  }

  // 0 at the approach radius, 1 on the marker. Drives height and opacity
  // directly so the growth tracks the pointer instead of easing behind it.
  function approach(key) {
    if (nearest?.key !== key) return 0;
    return Math.max(0, 1 - nearest.distance / APPROACH_PX);
  }

  const armed = (key) => nearest?.key === key && nearest.distance <= TOOLTIP_PX;

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
        {#if hover}
          <!-- Hover is the mini summary's layout, not the plain bar's: the bar
               clips its overflow to keep the fill inside its rounded ends, and
               hit-testing follows that clip — so the marker and the proximity
               target hang off a wrapper as siblings of the bar, letting the
               marker grow past the bar's 6px and the target span more than
               those 6px. The bar is visual only here; the target owns every
               pointer interaction, peek included, exactly as the mini's does. -->
          <div class="hover-bar-cell">
            <div class="bar">
              <!-- Informational windows never colour by threshold: they don't
                   gate anything, so red would be misleading. -->
              <div
                class="fill {w.informational ? 'muted' : barClass(w.used_pct)}"
                style="width: {Math.max(0, Math.min(w.used_pct, 100))}%"
              ></div>
            </div>
            {#if progress != null}
              <!-- Decorative and still aria-hidden: the row above already
                   shows the percentage and the "resets in …" countdown as
                   visible text, so the tooltip is a redundant enhancement —
                   a second reading of what the row says, never the sole
                   channel. Height is set inline per pointer position; only
                   `left` eases, so the growth tracks the pointer instead of
                   lagging 0.4s behind it. -->
              <i
                class="period-mark"
                style="left: {progress * 100}%; height: calc(var(--hover-bar-h) + {approach(w.label) * 10}px); opacity: {0.8 + approach(w.label) * 0.2}"
                aria-hidden="true"
              ></i>
              <!-- Deliberately pointer-only and out of the tab order: the
                   peek is a redundant enhancement, the marker it moves is
                   aria-hidden, and the tooltip repeats the row's own text.
                   `data-tip` rather than `title`: the OS draws `title` after
                   a delay of its own, which is too slow to scrub against.
                   Leaving clears the proximity *and* the peek, so a leave can
                   never strand a grown marker, an armed tooltip, or a held
                   peek. -->
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <span
                class="hover-bar-target"
                onpointermove={(e) => trackPointer(e, w.label, progress)}
                onpointerleave={() => {
                  nearest = null;
                  peek = null;
                }}
                onpointerdown={() => (peek = w.label)}
                onpointerup={releasePeek}
                onpointercancel={releasePeek}
                oncontextmenu={(e) => e.preventDefault()}
                data-tip={periodTooltip(w, progress, now)}
                data-armed={armed(w.label) ? '' : null}
              ></span>
            {/if}
          </div>
        {:else}
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
        {/if}
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
