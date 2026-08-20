<script>
  // Mobile shell: opens directly to the usage list, no window/tray concepts.
  // Issue #108 proved the foreground path for one hardcoded provider
  // (OpenRouter); issue #109 generalizes onboarding and account CRUD to every
  // direct-HTTPS pasted-key provider — Claude/Codex/Grok (OAuth) and Hermes
  // (cookie/SSH) stay excluded on Android per docs/adr/0006-…. A first run
  // has no accounts at all (Rust seeds an empty config — see
  // `Config::mobile_first_run_default` and `src-tauri/src/mobile.rs`'s
  // `run()`), so the settings view doubles as onboarding: the empty list
  // *is* the "pick a provider" prompt.
  import { onMount } from 'svelte';
  import UsageCard from '../lib/shared/UsageCard.svelte';
  import Ordering from '../lib/shared/Ordering.svelte';
  import Thresholds from '../lib/shared/Thresholds.svelte';
  import SimpleKeyAccount from '../lib/shared/SimpleKeyAccount.svelte';
  import OAuthAccount from './OAuthAccount.svelte';
  import {
    getSnapshots,
    setConfig,
    setSecret,
    hasSecret,
    clearSecret,
    refreshNow,
    testProvider,
    listen,
    startClaudeSignin,
    finishClaudeSignin,
    startCodexSignin,
    pollCodexSignin,
    cancelSignin,
    getPendingSignins,
  } from '../lib/host.js';
  import { runCredentialTest } from './credentialTest.js';

  // Every provider Android exposes after issue #110. Each carries a `mode`
  // that decides which settings UI it gets:
  //   - `oauth`: built-in sign-in (Claude PKCE, Codex device flow)
  //   - `cookie`: pasted session cookie (Hermes)
  //   - `key`: pasted API key (everything else)
  // CLI files, local commands, SSH and Tailscale are intentionally absent.
  const PROVIDERS = [
    { id: 'claude', name: 'Claude', mode: 'oauth', note: 'Built-in browser sign-in with your Anthropic account.' },
    { id: 'codex', name: 'Codex', mode: 'oauth', note: 'Built-in device-flow sign-in with your OpenAI account.' },
    { id: 'openrouter', name: 'OpenRouter', mode: 'key', secretLabel: 'API key', note: 'Create a key at openrouter.ai/keys. Optional monthly budget tracks this month’s spend against your target.' },
    { id: 'elevenlabs', name: 'ElevenLabs', mode: 'key', secretLabel: 'API key', note: 'Create a key at elevenlabs.io/app/settings/api-keys.' },
    { id: 'firecrawl', name: 'Firecrawl', mode: 'key', secretLabel: 'API key', note: 'Create a key at firecrawl.dev/app/api-keys.' },
    { id: 'deepseek', name: 'DeepSeek', mode: 'key', secretLabel: 'API key', note: 'Create a key at platform.deepseek.com/api_keys.' },
    { id: 'moonshot', name: 'Moonshot', mode: 'key', secretLabel: 'API key', note: 'Create a key at platform.kimi.ai. Keys are platform-specific: a platform.kimi.com key needs its Balance URL changed to that host, or it returns 401.' },
    { id: 'venice', name: 'Venice', mode: 'key', secretLabel: 'API key', note: 'Create a key at venice.ai. Reports USD and DIEM balances; pick which one heads the card below.' },
    { id: 'onehop', name: 'OneHop', mode: 'key', secretLabel: 'API key', note: 'Create a key in the OneHop console. Gateway wallet balance.' },
    { id: 'fireworks', name: 'Fireworks', mode: 'key', secretLabel: 'API key', note: 'Create a key at fireworks.ai/account/api-keys. Needs the account ID too. Reports spend, not a balance: set a monthly budget to see it as a percentage.' },
    { id: 'anthropic_admin', name: 'Anthropic Admin', mode: 'key', secretLabel: 'Admin API key', note: 'Needs an sk-ant-admin key from Console → Settings → Admin keys, not a normal API key. Shows organization spend this month.' },
    { id: 'openai_admin', name: 'OpenAI Admin', mode: 'key', secretLabel: 'Admin API key', note: 'Needs an organization Admin key from platform.openai.com/settings/organization/admin-keys, not a normal API key. Shows organization spend this month.' },
    { id: 'hermes', name: 'Hermes Portal', mode: 'cookie', secretLabel: 'Portal session cookie', note: 'Paste a portal.nousresearch.com session cookie. No local executable, SSH or Tailscale is used.' },
  ];
  const providerInfo = (kind) => PROVIDERS.find((p) => p.id === kind) ?? { id: kind, name: kind, mode: 'key', secretLabel: 'API key', note: '' };

  // Static headline choices per kind, same source data as
  // Settings.svelte's `metricOptions` — kept in sync there and here since
  // mobile does not share that component.
  const KNOWN_HEADLINES = {
    claude: [{ id: 'window:five_hour', label: '5-hour' }, { id: 'window:weekly', label: 'Weekly' }],
    codex: [{ id: 'window:five_hour', label: '5-hour' }, { id: 'window:weekly', label: 'Weekly' }],
    openrouter: [{ id: 'credits', label: 'Credit balance' }, { id: 'window:monthly_spend', label: 'Monthly' }],
    elevenlabs: [{ id: 'window:monthly_credits', label: 'Monthly credits' }],
    firecrawl: [{ id: 'window:monthly_credits', label: 'Monthly credits' }],
    deepseek: [{ id: 'credits', label: 'Credit balance' }],
    moonshot: [{ id: 'credits', label: 'Credit balance' }],
    venice: [{ id: 'credits', label: 'Credit balance' }],
    onehop: [{ id: 'credits', label: 'Credit balance' }],
    fireworks: [{ id: 'window:monthly_spend', label: 'Monthly' }, { id: 'credits', label: 'Spend this month' }],
    anthropic_admin: [{ id: 'window:monthly_spend', label: 'Monthly' }, { id: 'credits', label: 'Spend this month' }],
    openai_admin: [{ id: 'window:monthly_spend', label: 'Monthly' }, { id: 'credits', label: 'Spend this month' }],
    hermes: [{ id: 'credits', label: 'Credit balance' }],
  };

  let view = $state('list'); // 'list' | 'settings'
  let snapshots = $state([]);
  let config = $state(null);
  let refreshing = $state(false);
  let secretStored = $state({});
  let secretInputs = $state({});
  let testResults = $state({});
  let openHeadlineFor = $state('');
  let expanded = $state({});
  let addingAccount = $state(false);
  let newKind = $state('openrouter');
  let newName = $state('');
  let pendingSignins = $state([]);

  function headlineOptionsFor(id, account) {
    const kind = account.kind ?? id;
    const known = KNOWN_HEADLINES[kind] ?? [];
    const live = snapshots.find((s) => s.provider_id === id)?.windows ?? [];
    const choices = [...known];
    for (const w of live) {
      if (w.metric_id) choices.push({ id: `window:${w.metric_id}`, label: w.label });
    }
    for (const metric of account.mini_summary_metrics ?? []) {
      choices.push({ id: metric, label: metric.replace(/^window:/, '') });
    }
    return choices.filter((c, i, all) => all.findIndex((o) => o.id === c.id) === i);
  }

  function newAccount(kind, label) {
    const info = providerInfo(kind);
    const settings = {};
    if (info.mode === 'oauth') {
      settings.auth_mode = 'oauth';
    } else if (info.mode === 'cookie') {
      settings.source = 'cookie';
    }
    return {
      kind,
      label: label || null,
      enabled: true,
      in_tray: true,
      thresholds: null,
      alerts: null,
      low_balance_warn: null,
      mini_summary_metric: null,
      mini_summary_metrics: null,
      tray_metric: null,
      // All seven on: identical to the field being absent, which is what
      // quota-core's `UsageSchedule::default` reproduces on load. The mobile
      // schedule editor is issue #123; this keeps the account shape consistent
      // with desktop so a config written on either side loads the same on the
      // other.
      usage_schedule: { monday: true, tuesday: true, wednesday: true, thursday: true, friday: true, saturday: true, sunday: true },
      settings,
    };
  }

  async function addAccount() {
    const n = Object.entries(config.providers).filter(([id, p]) => (p.kind ?? id) === newKind).length + 1;
    let key = `${newKind}#${n}`;
    while (config.providers[key]) key = `${newKind}#${Number(key.split('#')[1]) + 1}`;
    config.providers[key] = newAccount(newKind, newName.trim());
    // Seed the per-account maps before the row renders — `expanded` and
    // `secretInput` are two-way bound into `SimpleKeyAccount`, and a bound
    // prop that starts `undefined` while also having a fallback default
    // throws (Svelte can't safely reconcile "unset" with "bound"). See the
    // same seeding in `onMount` and `syncAccountState` below.
    expanded[key] = true;
    secretInputs[key] = '';
    await persist();
    newName = '';
    addingAccount = false;
  }

  // Backfills the per-account maps for every account currently in `config` —
  // needed after the initial load and whenever another instance's `set_config`
  // replaces `config` out from under us (the `listen('config', …)` handler),
  // since either can introduce ids this instance never seeded via `addAccount`.
  function syncAccountState() {
    for (const id of Object.keys(config.providers)) {
      if (secretInputs[id] === undefined) secretInputs[id] = '';
      if (expanded[id] === undefined) expanded[id] = false;
    }
  }

  async function persist() {
    for (const [id, value] of Object.entries(secretInputs)) {
      // A pasted key for an account removed before Save must not resurrect a
      // secret entry for an id that no longer exists in the config.
      if (!value || !config.providers[id]) continue;
      await setSecret(id, value);
      secretStored[id] = true;
      secretInputs[id] = '';
    }
    await setConfig($state.snapshot(config));
  }

  // Every key-mode provider tests through this one lifecycle (issue #135), so
  // pending/error cleanup cannot diverge per provider. The heavy lifting —
  // store-before-request, bounded wait, failure categorization, always-
  // terminal result — lives in `runCredentialTest`; here we only translate its
  // outcome into the per-account UI state the row is bound to.
  async function test(id) {
    testResults[id] = { pending: true };
    const info = providerInfo(config.providers[id]?.kind ?? id);
    const result = await runCredentialTest({
      id,
      pastedSecret: secretInputs[id],
      snapshotConfig: $state.snapshot(config),
      secretLabel: info.secretLabel ?? 'API key',
      host: { setConfig, setSecret, testProvider },
    });
    // A stored key is stored whether or not the provider accepted it; only a
    // fully successful test consumes the pasted value from the form.
    if (result.storedSecret) secretStored[id] = true;
    if (result.clearInput) secretInputs[id] = '';
    testResults[id] = result.ok
      ? { ok: true, msg: 'ok' }
      : { ok: false, msg: result.msg, category: result.category };
  }

  async function removeAccount(id) {
    const info = providerInfo(config.providers[id].kind ?? id);
    const secretKey = info.mode === 'oauth' ? `${id}_oauth` : id;
    await clearSecret(secretKey);
    await cancelSignin(id);
    delete secretStored[id];
    delete secretInputs[id];
    delete config.providers[id];
    await persist();
  }

  function moveAccount(id, direction) {
    const entries = Object.entries(config.providers);
    const index = entries.findIndex(([key]) => key === id);
    const target = index + direction;
    if (index < 0 || target < 0 || target >= entries.length) return;
    const [entry] = entries.splice(index, 1);
    entries.splice(target, 0, entry);
    config.providers = Object.fromEntries(entries);
  }

  async function refresh() {
    refreshing = true;
    await refreshNow();
    setTimeout(() => (refreshing = false), 1200);
  }

  async function loadPending() {
    pendingSignins = await getPendingSignins().catch(() => []);
  }

  function pendingFor(id) {
    return pendingSignins.find((p) => p.provider_key === id) ?? null;
  }

  async function startClaude(id) {
    const url = await startClaudeSignin(id);
    await loadPending();
    if (window.__TAURI__?.opener?.openUrl) {
      window.__TAURI__.opener.openUrl(url);
    } else {
      window.open(url, '_blank');
    }
  }

  async function finishClaude(id, code) {
    await finishClaudeSignin(id, code);
    await loadPending();
    secretStored[id] = true;
    await refresh();
  }

  async function startCodex(id) {
    await startCodexSignin(id);
    await loadPending();
  }

  async function pollCodex(id) {
    const result = await pollCodexSignin(id);
    if (result === 'complete') {
      await loadPending();
      secretStored[id] = true;
      await refresh();
    }
  }

  async function cancelClaudeOrCodex(id) {
    await cancelSignin(id);
    await loadPending();
  }

  onMount(() => {
    getSnapshots().then(async (initial) => {
      snapshots = initial.snapshots;
      config = initial.config;
      syncAccountState();
      for (const id of Object.keys(config.providers)) {
        const info = providerInfo(config.providers[id].kind ?? id);
        const secretKey = info.mode === 'oauth' ? `${id}_oauth` : id;
        secretStored[id] = await hasSecret(secretKey).catch(() => false);
      }
      await loadPending();
      // The debug-only CI OpenRouter seed lives in Rust `setup()` (see
      // src-tauri/src/mobile.rs), not here: the Android webview reloads once
      // during startup and drops in-flight invoke callbacks, which repeatedly
      // cut a JS-side seed's persist→refresh chain short. Seeding in Rust
      // before the webview loads means getSnapshots() above already returns the
      // seeded account and its snapshot, with nothing to do here.
    });
    const unlisten = [];
    listen('snapshots', (e) => (snapshots = e.payload)).then((u) => unlisten.push(u));
    listen('config', (e) => {
      config = e.payload;
      syncAccountState();
    }).then((u) => unlisten.push(u));
    return () => unlisten.forEach((u) => u());
  });
</script>

<main class="mobile">
  <header class="mobile-header">
    <span class="title">Quota Widget</span>
    <span class="spacer"></span>
    {#if view === 'list'}
      <button class="icon" title="Refresh now" class:spin={refreshing} onclick={refresh}>⟳</button>
      <button class="icon" title="Settings" onclick={() => (view = 'settings')}>⚙</button>
    {:else}
      <button class="icon" title="Back" onclick={() => (view = 'list')}>←</button>
    {/if}
  </header>

  {#if !config}
    <p class="empty">Loading…</p>
  {:else if view === 'list'}
    <div class="cards">
      {#if snapshots.length === 0}
        <p class="empty">
          No providers enabled yet — open <button class="link" onclick={() => (view = 'settings')}>Settings</button> to add one.
        </p>
      {:else}
        {#each snapshots as snap (snap.provider_id)}
          <UsageCard {snap} schedule={config?.providers?.[snap.provider_id]?.usage_schedule} />
        {/each}
      {/if}
    </div>
  {:else}
    <div class="settings mobile-settings">
      <section>
        <h2>Providers</h2>
        {#if Object.keys(config.providers).length === 0}
          <p class="note">No accounts yet — add one to get started.</p>
        {/if}
        {#if addingAccount}
          <div class="add-account">
            <label class="field">Account name (optional)
              <input placeholder="Account name (optional)" bind:value={newName} />
            </label>
            <label class="field">Provider
              <select bind:value={newKind}>{#each PROVIDERS as p}<option value={p.id}>{p.name}</option>{/each}</select>
            </label>
            <div class="add-account-actions">
              <button onclick={addAccount}>Add account</button>
              <button onclick={() => (addingAccount = false)}>Cancel</button>
            </div>
          </div>
        {:else}
          <button class="add-account-toggle" onclick={() => (addingAccount = true)}>+ Add account</button>
        {/if}
        {#each Object.entries(config.providers) as [id, account], index (id)}
          {@const info = providerInfo(account.kind ?? id)}
          {@const pending = pendingFor(id)}
          {#if info.mode === 'oauth'}
            <OAuthAccount
              {id}
              kind={account.kind ?? id}
              bind:account={config.providers[id]}
              providerName={info.name}
              providerNote={info.note}
              {pending}
              secretStored={secretStored[id] ?? false}
              headlineOptions={headlineOptionsFor(id, account)}
              headlineOpen={openHeadlineFor === id}
              onToggleHeadline={() => (openHeadlineFor = openHeadlineFor === id ? '' : id)}
              bind:expanded={expanded[id]}
              onSignIn={() => (account.kind ?? id) === 'claude' ? startClaude(id) : startCodex(id)}
              onCompleteClaude={(code) => finishClaude(id, code)}
              onPollCodex={() => pollCodex(id)}
              onCancel={() => cancelClaudeOrCodex(id)}
              onRemove={() => removeAccount(id)}
              onMoveUp={() => moveAccount(id, -1)}
              onMoveDown={() => moveAccount(id, 1)}
              canMoveUp={index > 0}
              canMoveDown={index < Object.keys(config.providers).length - 1}
            />
          {:else}
            <SimpleKeyAccount
              {id}
              kind={account.kind ?? id}
              bind:account={config.providers[id]}
              providerName={info.name}
              providerNote={info.note}
              secretLabel={info.secretLabel}
              secretStored={secretStored[id] ?? false}
              bind:secretInput={secretInputs[id]}
              headlineOptions={headlineOptionsFor(id, account)}
              headlineOpen={openHeadlineFor === id}
              onToggleHeadline={() => (openHeadlineFor = openHeadlineFor === id ? '' : id)}
              bind:expanded={expanded[id]}
              testResult={testResults[id] ?? null}
              onTest={() => test(id)}
              onClearSecret={async () => { await clearSecret(id); secretStored[id] = false; }}
              onRemove={() => removeAccount(id)}
              onMoveUp={() => moveAccount(id, -1)}
              onMoveDown={() => moveAccount(id, 1)}
              canMoveUp={index > 0}
              canMoveDown={index < Object.keys(config.providers).length - 1}
            />
          {/if}
        {/each}
      </section>
      <section>
        <h2>Ordering</h2>
        <Ordering bind:sortOrder={config.sort_order} bind:sortBasis={config.sort_basis} />
      </section>
      <section>
        <h2>Thresholds</h2>
        <Thresholds bind:thresholds={config.thresholds} />
      </section>
      <div class="settings-footer">
        <button class="primary" onclick={async () => { await persist(); view = 'list'; }}>Save</button>
      </div>
    </div>
  {/if}
</main>
