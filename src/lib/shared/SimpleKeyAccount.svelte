<script>
  // Account management for a pasted-API-key provider — no OAuth, no CLI
  // fallback. Covers every direct-HTTPS pasted-key adapter Android exposes
  // (issue #109): OpenRouter, ElevenLabs, Firecrawl, DeepSeek, Moonshot,
  // Venice, OneHop, Fireworks, Anthropic Admin, OpenAI Admin. `kind` selects
  // the small set of per-provider extra fields desktop's Settings.svelte
  // renders inline for the same adapters — ported here verbatim rather than
  // redesigned, so mobile and desktop agree on what each provider needs.
  //
  // `account` is mutated in place (label, enabled, tray/headline picks,
  // low_balance_warn, settings.*); secret storage and testing go through the
  // caller's callbacks, since those are IPC calls the host owns.
  import HeadlineSelection from './HeadlineSelection.svelte';
  import ScheduleSelection from './ScheduleSelection.svelte';

  let {
    id,
    kind,
    account = $bindable(),
    providerName,
    providerNote = '',
    secretLabel = 'API key',
    secretStored = false,
    secretInput = $bindable(''),
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

  // Mirrors Settings.svelte's inline `{#if p.id === '…'}` blocks exactly —
  // see src/lib/Settings.svelte around the account row.
  const showAccountId = $derived(kind === 'fireworks');
  const showBalanceUrl = $derived(kind === 'moonshot');
  const showCurrency = $derived(kind === 'venice');
  const showBudget = $derived(
    ['fireworks', 'anthropic_admin', 'openai_admin', 'openrouter'].includes(kind),
  );
  const showLowBalance = $derived(
    ['openrouter', 'deepseek', 'moonshot', 'venice'].includes(kind),
  );

  account.settings ??= {};
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
    <ScheduleSelection bind:account />
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
    {#if showAccountId}
      <label class="field">Account ID
        <input type="text" placeholder="required — from your Fireworks account page" bind:value={account.settings.account_id} />
      </label>
    {/if}
    {#if showBudget}
      <label class="field">Monthly budget (optional)
        <input type="number" step="any" placeholder="USD — set to see spend as a percentage" bind:value={account.settings.monthly_budget} />
      </label>
    {/if}
    {#if showBalanceUrl}
      <label class="field">Balance URL
        <input type="text" placeholder="default: https://api.moonshot.ai/v1/users/me/balance" bind:value={account.settings.balance_url} />
      </label>
    {/if}
    {#if showCurrency}
      <label class="field">Headline balance
        <select bind:value={account.settings.balance_currency}>
          <option value="USD">USD</option>
          <option value="DIEM">DIEM</option>
        </select>
      </label>
    {/if}
    {#if showLowBalance}
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
