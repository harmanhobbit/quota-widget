<script>
  // Built-in OAuth sign-in UI for Claude (PKCE paste-back) and Codex (device
  // flow) on Android. Keeps the Rust host as the source of truth for pending
  // sign-in state; the UI just drives the user through the browser and pastes
  // or polls until the host reports the tokens are stored.
  import HeadlineSelection from '../lib/shared/HeadlineSelection.svelte';

  let {
    id,
    kind,
    account = $bindable(),
    providerName,
    providerNote = '',
    pending = null,
    secretStored = false,
    headlineOptions,
    headlineOpen = false,
    onToggleHeadline,
    expanded = $bindable(true),
    onSignIn,
    onCompleteClaude,
    onPollCodex,
    onCancel,
    onRemove,
    onMoveUp,
    onMoveDown,
    canMoveUp = false,
    canMoveDown = false,
  } = $props();

  let claudeCode = $state('');
  let polling = $state(false);
  let lastError = $state('');

  async function signIn() {
    lastError = '';
    try {
      await onSignIn();
    } catch (e) {
      lastError = e?.message ?? String(e);
    }
  }

  async function completeClaude() {
    lastError = '';
    try {
      await onCompleteClaude(claudeCode.trim());
      claudeCode = '';
    } catch (e) {
      lastError = e?.message ?? String(e);
    }
  }

  async function pollCodex() {
    lastError = '';
    polling = true;
    try {
      await onPollCodex();
    } catch (e) {
      lastError = e?.message ?? String(e);
    } finally {
      polling = false;
    }
  }

  function openBrowser(url) {
    if (window.__TAURI__?.opener?.openUrl) {
      window.__TAURI__.opener.openUrl(url);
    } else {
      window.open(url, '_blank');
    }
  }

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

    {#if secretStored}
      <p class="note good">Signed in.</p>
      <div class="provider-footer">
        <button class="small" onclick={onRemove}>Remove account</button>
      </div>
    {:else if pending}
      {#if kind === 'claude'}
        <p class="note">
          Authorize in the browser, then paste the <code>code#state</code> string shown on the confirmation page.
        </p>
        <div class="row">
          <input placeholder="code#state" bind:value={claudeCode} />
          <button onclick={completeClaude}>Complete sign-in</button>
        </div>
      {:else if kind === 'codex'}
        <p class="note">
          Open <button class="link" onclick={() => openBrowser(pending.verification_url)}>{pending.verification_url}</button> and enter:
        </p>
        <div class="code-box">{pending.user_code}</div>
        <div class="row">
          <button onclick={pollCodex} disabled={polling}>
            {polling ? 'Checking…' : 'I entered the code — check'}
          </button>
          <button class="small" onclick={() => openBrowser(pending.verification_url)}>Open browser</button>
        </div>
      {/if}
      <div class="provider-footer">
        <button class="small" onclick={onCancel}>Cancel sign-in</button>
      </div>
    {:else}
      <div class="row">
        <button onclick={signIn}>Sign in with {providerName}</button>
      </div>
      <div class="provider-footer">
        <button class="small" onclick={onRemove}>Remove account</button>
      </div>
    {/if}

    {#if lastError}
      <p class="test bad">{lastError}</p>
    {/if}
  {/if}
</div>

<style>
  .code-box {
    font-family: ui-monospace, monospace;
    font-size: 1.25rem;
    letter-spacing: 0.05em;
    padding: 0.75rem;
    background: var(--surface-2, #f0f0f0);
    border-radius: 0.375rem;
    text-align: center;
    margin: 0.5rem 0;
    user-select: all;
  }
</style>
