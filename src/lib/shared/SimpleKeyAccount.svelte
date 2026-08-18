<script>
  // Account management for a plain pasted-API-key provider — no OAuth, no CLI
  // fallback, no admin-key or provider-specific extra fields. Covers
  // OpenRouter, ElevenLabs, Firecrawl and DeepSeek's shape today. A provider
  // with extra settings (Fireworks' account id, Venice's currency pick, …)
  // isn't this component's job — it stays wherever the host composes its own
  // fields alongside this one, or is left out of this component entirely.
  //
  // `account` is mutated in place (label, enabled, tray/headline picks,
  // low_balance_warn); secret storage and testing go through the caller's
  // callbacks, since those are IPC calls the host owns.
  import HeadlineSelection from './HeadlineSelection.svelte';

  let {
    id,
    account = $bindable(),
    providerName,
    providerNote = '',
    secretLabel = 'API key',
    secretStored = false,
    secretInput = $bindable(''),
    lowBalanceWarn = false,
    headlineOptions,
    headlineOpen = false,
    onToggleHeadline,
    expanded = $bindable(true),
    testResult = null,
    onTest,
    onClearSecret,
    onRemove,
    onMoveUp,
    onMoveDown,
    canMoveUp = false,
    canMoveDown = false,
  } = $props();
</script>

<div class="provider">
  <div class="provider-header row" class:collapsed={!expanded}>
    <button
      class="provider-disclosure"
      aria-expanded={expanded === true}
      onclick={() => (expanded = !expanded)}
    ><span class="chevron" class:open={expanded}>▸</span> <strong>{account.label ?? providerName}</strong></button>
    <span class="spacer"></span>
    <label class="inline">
      <input type="checkbox" bind:checked={account.enabled} />
      Enabled
    </label>
    {#if onMoveUp}
      <button class="small" title="Move account up" aria-label={`Move ${account.label ?? providerName} up`} disabled={!canMoveUp} onclick={onMoveUp}>↑</button>
    {/if}
    {#if onMoveDown}
      <button class="small" title="Move account down" aria-label={`Move ${account.label ?? providerName} down`} disabled={!canMoveDown} onclick={onMoveDown}>↓</button>
    {/if}
  </div>
  {#if expanded}
    {#if providerNote}<p class="note">{providerNote}</p>{/if}
    <label class="field">Account name <input maxlength="40" bind:value={account.label} placeholder={providerName} /></label>
    <HeadlineSelection bind:account options={headlineOptions} open={headlineOpen} onToggle={onToggleHeadline} />
    <div class="row">
      <input
        type="password"
        placeholder={secretStored ? `${secretLabel} stored — paste to replace` : `Paste ${secretLabel}`}
        bind:value={secretInput}
      />
      {#if secretStored}
        <button class="small" onclick={onClearSecret}>Clear</button>
      {/if}
    </div>
    {#if lowBalanceWarn}
      <div class="row">
        <label class="inline">Low-balance warning at
          <input type="number" step="any" class="num" bind:value={account.low_balance_warn} placeholder="off" />
        </label>
      </div>
    {/if}
    {#if testResult}
      <p class="test {testResult.ok ? 'good' : 'bad'}">
        {testResult.pending ? 'testing…' : testResult.msg}
      </p>
    {/if}
    <div class="provider-footer">
      <button class="small" onclick={onTest}>Test</button>
      <span class="spacer"></span>
      <button class="small" onclick={onRemove}>Remove account</button>
    </div>
  {/if}
</div>
