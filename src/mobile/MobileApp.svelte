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
  import CredentialTransfer from './CredentialTransfer.svelte';
  import {
    getSnapshots,
    setConfig,
    setSecret,
    hasSecret,
    clearSecret,
    refreshNow,
    refreshManual,
    testProvider,
    listen,
    startClaudeSignin,
    finishClaudeSignin,
    startCodexSignin,
    pollCodexSignin,
    cancelSignin,
    getPendingSignins,
    getOpener,
  } from '../lib/host.js';
  import { runCredentialTest } from './credentialTest.js';
  import { foregroundRefresh } from './foregroundRefresh.js';
  import { openExternal } from './browserHandoff.js';

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
  // Authorize URL for a Claude sign-in started *this session* — kept only in
  // JS memory, never persisted (issue #160). A sign-in resumed after process
  // death has no entry here, since the one-time PKCE challenge behind the
  // URL is regenerated on every start; that's the case the paste-only UI
  // stays for.
  let claudeUrls = $state({});

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
      // quota-core's `UsageSchedule::default` reproduces on load. Editable per
      // account via ScheduleSelection; the shape is kept consistent with
      // desktop so a config written on either side loads the same on the other.
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
      // Forward-compat: a config written by a pre-schedule build (or by a
      // desktop that has never touched this account) loads with the field
      // absent. All-seven is the pre-schedule default and what quota-core's
      // `UsageSchedule::default` reproduces, so seed it before the schedule
      // editor binds `usage_schedule[day]` — an undefined object would throw.
      config.providers[id].usage_schedule ??= {
        monday: true, tuesday: true, wednesday: true, thursday: true,
        friday: true, saturday: true, sunday: true,
      };
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
    delete claudeUrls[id];
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

  // The header button is the *manual* refresh (issue #111): durable one-time
  // work on the native host, so the fetch still lands if the app is dismissed
  // right after the tap; the cards update when the worker announces the new
  // read model with the `snapshots` event. Sign-in completions and imports
  // refresh through this too — their credentials are already persisted, so
  // the durable pass reads them from storage like any background refresh.
  // The foreground visibility loop stays on the in-process `refreshNow`.
  async function refresh() {
    refreshing = true;
    await refreshManual();
    setTimeout(() => (refreshing = false), 1200);
  }

  async function loadPending() {
    pendingSignins = await getPendingSignins().catch(() => []);
  }

  // Refills `secretStored` for every account in the current config — after the
  // initial load and after a credential import lands accounts whose keys came
  // with them, so their rows show the stored state rather than a paste field.
  async function scanSecrets() {
    for (const id of Object.keys(config.providers)) {
      const info = providerInfo(config.providers[id].kind ?? id);
      const secretKey = info.mode === 'oauth' ? `${id}_oauth` : id;
      secretStored[id] = await hasSecret(secretKey).catch(() => false);
    }
  }

  // A credential import landed accounts (issue #152): re-read the persisted
  // configuration rather than wait for the `config` event (which may not have
  // arrived yet), re-scan which accounts hold secrets, and refresh so the
  // newly stored pasted-key accounts read immediately. The imported OAuth
  // shells have no secret and stay in provider onboarding — their Sign in
  // buttons are already how the list renders them.
  async function handleImported() {
    const initial = await getSnapshots();
    config = initial.config;
    syncAccountState();
    await scanSecrets();
    await refresh();
  }

  function pendingFor(id) {
    return pendingSignins.find((p) => p.provider_key === id) ?? null;
  }

  async function startClaude(id) {
    const url = await startClaudeSignin(id);
    claudeUrls[id] = url;
    await loadPending();
    // No window.open fallback: a launch failure must surface as an error,
    // not disappear silently (issue #160). The URL is already stored above
    // so the pending UI can still show it as a copyable manual path.
    await openExternal(url, { opener: getOpener() });
  }

  async function finishClaude(id, code) {
    await finishClaudeSignin(id, code);
    await loadPending();
    secretStored[id] = true;
    delete claudeUrls[id];
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
    delete claudeUrls[id];
  }

  // Foreground refresh (issue #111): refresh once on entry, repeat at the
  // configured interval while visible, and stop when the app is backgrounded.
  // The cadence lives in `foregroundRefresh` so it is unit-tested; here we only
  // bind it to the document's visibility. Background refresh while the app is
  // hidden is the OS's job (the best-effort background refresh target), never
  // this loop's.
  const fg = foregroundRefresh({
    refresh: () => refreshNow(),
    intervalSecs: () => config?.poll_interval_secs ?? 60,
  });

  function onVisibilityChange() {
    if (document.visibilityState === 'visible') fg.enter();
    else fg.leave();
  }

  onMount(() => {
    getSnapshots().then(async (initial) => {
      snapshots = initial.snapshots;
      config = initial.config;
      syncAccountState();
      await scanSecrets();
      await loadPending();
      // The foreground cadence reads the configured interval when it (re)starts,
      // so re-enter now that the config has actually landed: the mount-time
      // enter() below ran while `config` was still null and could only fall
      // back to the 60s default — which would otherwise persist for the whole
      // session and never follow the configured interval. Restarting the
      // cadence is exactly what a visibility change does; this just does it
      // once the real interval is known.
      if (document.visibilityState !== 'hidden') fg.enter();
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
      // A saved poll-interval change takes effect on the visible loop
      // immediately, not at the next visibility change (issue #111's
      // "continue at the configured interval while visible").
      if (document.visibilityState !== 'hidden') fg.enter();
    }).then((u) => unlisten.push(u));
    // The durable worker's provenance marker (issue #111): emitted only by the
    // WorkManager worker path — never by the foreground loop or entry refresh —
    // so this line attributes an update to background work. The emulator check
    // asserts exactly this marker in logcat as its delivery proof: it is
    // produced *by this webview* reacting to the worker's push, not by any
    // other refresh path.
    listen('worker-refresh', () =>
      console.log('[quota-widget] worker refresh delivered to this webview'),
    ).then((u) => unlisten.push(u));

    // Start the foreground loop if we launched visible (the usual case), and
    // follow the app in and out of the foreground thereafter. Rust already ran
    // one refresh at setup before the webview loaded, but entering here gives
    // the webview its own immediate refresh and establishes the repeat.
    document.addEventListener('visibilitychange', onVisibilityChange);
    if (document.visibilityState !== 'hidden') fg.enter();

    return () => {
      document.removeEventListener('visibilitychange', onVisibilityChange);
      fg.leave();
      unlisten.forEach((u) => u());
    };
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
              claudeUrl={claudeUrls[id] ?? null}
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
      <CredentialTransfer onImported={handleImported} />
      <section>
        <h2>Ordering</h2>
        <Ordering bind:sortOrder={config.sort_order} bind:sortBasis={config.sort_basis} />
      </section>
      <section>
        <h2>Thresholds</h2>
        <Thresholds bind:thresholds={config.thresholds} />
      </section>
      <section>
        <h2>Notifications</h2>
        <label class="inline">
          <input type="checkbox" bind:checked={config.alerts.toast} />
          Notify me when an account crosses a threshold
        </label>
        <p class="note">
          Alerts arrive as Android notifications. On Android 13 and later, Quota
          Widget asks for notification permission once — after your first account
          reads successfully, so the request has context. Declining is fine:
          refresh and widgets keep working, and you won't be asked again.
          Notifications are private, so the lock screen shows only generic text;
          the account and the figures appear once you unlock. To turn them back
          on later, open Android Settings → Apps → Quota&nbsp;Widget →
          Notifications.
        </p>
      </section>
      <section>
        <h2>Background refresh</h2>
        <p class="note">
          While open, Quota Widget refreshes as soon as you switch to it and
          then every {Math.max(15, config.poll_interval_secs ?? 60)} seconds until
          you leave. In the background it aims to refresh roughly every 15
          minutes — a best-effort target, not a guarantee: Android decides when
          background work actually runs, so figures can be older than that.
        </p>
      </section>
      <div class="settings-footer">
        <button class="primary" onclick={async () => { await persist(); view = 'list'; }}>Save</button>
      </div>
    </div>
  {/if}
</main>
