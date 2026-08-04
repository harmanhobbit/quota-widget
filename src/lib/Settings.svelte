<script>
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';

  let { onclose, initialConfig, snapshots = [] } = $props();

  const PROVIDERS = [
    { id: 'claude', name: 'Claude', secret: null, note: 'Uses the Claude Code CLI login if present, or the built-in browser sign-in below.' },
    { id: 'codex', name: 'Codex', secret: null, note: 'Uses the Codex CLI login if present, or the built-in device sign-in below.' },
    { id: 'openrouter', name: 'OpenRouter', secret: 'API key', note: 'Create a key at openrouter.ai/keys.' },
    { id: 'elevenlabs', name: 'ElevenLabs', secret: 'API key', note: 'Create a key at elevenlabs.io/app/settings/api-keys.' },
    { id: 'hermes', name: 'Hermes Portal', secret: 'Session cookie', note: 'Uses a hermes-agent login: local ~/.hermes/auth.json, or fetched from a remote machine over SSH (needs working key auth, e.g. via ssh-agent). Cookie paste is a last-resort fallback.' },
  ];
  const providerInfo = (kind) => PROVIDERS.find((p) => p.id === kind) ?? { id: kind, name: kind, secret: null, note: 'Unknown provider kind.' };

  // `initialConfig` arrives as a Svelte state proxy, which structuredClone
  // rejects. $state.snapshot is the proxy-aware deep clone.
  const settingsConfig = () => $state.snapshot(initialConfig);
  let config = $state(settingsConfig());
  let appVersion = $state('');
  let secretInputs = $state({});
  let secretStored = $state({});
  let testResults = $state({});
  let oauth = $state({});
  // Codex uses a device flow: we show a short code, the user types it in the
  // browser, and Rust emits `codex-oauth` when polling resolves.
  let codex = $state({});
  // Native Wayland can't honour always-on-top, so the popup sinks behind other
  // windows regardless of the click-away setting. Worth saying so in place.
  let onWayland = $state(false);
  let newKind = $state('claude');
  let newName = $state('');
  let addingAccount = $state(false);
  let saveError = $state('');
  let removalError = $state('');
  // Removal clears secrets before deleting the config entry. Save must wait
  // for that async work so a fast Remove → Save click cannot resurrect it.
  const pendingRemovals = new Set();

  const SORT_ORDERS = [
    { id: 'manual', label: 'Manual (my order)' },
    { id: 'usage_desc', label: 'Usage: high to low' },
    { id: 'usage_asc', label: 'Usage: low to high' },
    { id: 'expiry_soonest', label: 'Expiry: soonest first' },
    { id: 'expiry_furthest', label: 'Expiry: furthest first' },
  ];
  const SORT_BASES = [
    { id: 'icon', label: 'the number in the tray icon' },
    { id: 'worst_case', label: 'the worst window' },
  ];

  async function initialiseSettings() {
    // A config written before sorting existed has neither field; `undefined`
    // would leave the selects blank and then save `null` back to Rust.
    config.sort_order ??= 'manual';
    config.sort_basis ??= 'icon';
    // Normalize configured accounts only. Do not recreate removed defaults.
    for (const account of Object.values(config.providers)) {
      account.settings ??= {};
      account.in_tray ??= true;
      // Rust migrates these on load, but a config that reaches the UI another
      // way (an old cached payload) must not leave them undefined — `undefined`
      // and `null` mean different things to the pickers.
      account.mini_summary_metrics ??= null;
      account.tray_metric ??= null;
    }
    ensureFlows();
    invoke('app_version').then((version) => (appVersion = version)).catch(() => {});
    for (const [id, account] of Object.entries(config.providers)) {
      const p = providerInfo(account.kind ?? id);
      if (p.secret) secretStored[id] = await invoke('has_secret', { provider: id });
    }
    for (const [id, account] of Object.entries(config.providers)) {
      const kind = account.kind ?? id;
      if (kind === 'claude') oauthFor(id).signedIn = await invoke('has_secret', { provider: `${id}_oauth` });
      if (kind === 'codex') codexFor(id).signedIn = await invoke('has_secret', { provider: `${id}_oauth` });
    }
    onWayland = await invoke('on_wayland');
  }

  onMount(() => {
    void initialiseSettings();
    let unlisten;
    listen('codex-oauth', (e) => {
      const flow = codexFor(e.payload.provider);
      if (e.payload.ok) {
        flow.signedIn = true;
        flow.status = '';
        flow.userCode = '';
      } else {
        flow.status = e.payload.error;
      }
    }).then((stop) => (unlisten = stop));
    // App binds Escape to leaving Settings. Capture phase so an open headline
    // menu swallows the first press instead of the whole screen closing.
    const escape = (event) => {
      if (event.key === 'Escape' && openMetricMenu) {
        openMetricMenu = '';
        event.stopPropagation();
      }
    };
    window.addEventListener('keydown', escape, true);
    return () => {
      unlisten?.();
      window.removeEventListener('keydown', escape, true);
    };
  });

  const newOauthFlow = () => ({ url: '', code: '', status: '', signedIn: false });
  const newCodexFlow = () => ({ userCode: '', url: '', status: '', signedIn: false });

  function oauthFor(provider) {
    return (oauth[provider] ??= newOauthFlow());
  }

  function codexFor(provider) {
    return (codex[provider] ??= newCodexFlow());
  }

  // The markup reads these from inside `{@const}`, which compiles to a derived.
  // Deriveds must not write to state, so these never create a missing entry —
  // they fall back to a throwaway blank flow. Entries are created eagerly by
  // `ensureFlows` instead.
  const oauthView = (provider) => oauth[provider] ?? newOauthFlow();
  const codexView = (provider) => codex[provider] ?? newCodexFlow();

  function ensureFlows() {
    for (const [id, account] of Object.entries(config.providers)) {
      const kind = account.kind ?? id;
      if (kind === 'claude') oauthFor(id);
      if (kind === 'codex') codexFor(id);
    }
  }

  async function codexStart(provider) {
    const flow = codexFor(provider);
    flow.status = 'waiting';
    try {
      const r = await invoke('codex_oauth_start', { provider });
      flow.userCode = r.user_code;
      flow.url = r.verification_url;
    } catch (e) {
      flow.status = String(e);
      flow.userCode = '';
    }
  }

  async function codexClear(provider) {
    await invoke('clear_secret', { provider: `${provider}_oauth` });
    codexFor(provider).signedIn = false;
  }

  async function oauthStart(provider) {
    const flow = oauthFor(provider);
    flow.status = '';
    flow.url = await invoke('claude_oauth_start', { provider });
  }

  async function oauthFinish(provider) {
    const flow = oauthFor(provider);
    try {
      await invoke('claude_oauth_finish', { code: flow.code, provider });
      flow.status = 'ok';
      flow.signedIn = true;
      flow.url = '';
      flow.code = '';
    } catch (e) {
      flow.status = String(e);
    }
  }

  async function oauthClear(provider) {
    await invoke('clear_secret', { provider: `${provider}_oauth` });
    oauthFor(provider).signedIn = false;
  }

  // Write settings without leaving the panel — used by Test, which needs the
  // pasted secret persisted before it runs.
  async function persist() {
    await Promise.all([...pendingRemovals]);
    if (removalError) throw new Error(removalError);
    for (const [id, value] of Object.entries(secretInputs)) {
      if (value) {
        await invoke('set_secret', { provider: id, value });
        secretStored[id] = true;
        secretInputs[id] = '';
      }
    }
    // Numeric fields arrive as strings from inputs; coerce before saving.
    config.poll_interval_secs = Math.max(15, Number(config.poll_interval_secs) || 60);
    config.thresholds.warn_pct = Number(config.thresholds.warn_pct) || 80;
    config.thresholds.critical_pct = Number(config.thresholds.critical_pct) || 95;
    for (const p of Object.values(config.providers)) {
      if (p.label != null) {
        p.label = p.label.trim();
        if (!p.label) throw new Error('Account name cannot be empty');
      }
      if (p.low_balance_warn === '' || p.low_balance_warn == null) p.low_balance_warn = null;
      else p.low_balance_warn = Number(p.low_balance_warn);
      // Drop empty settings values so Rust-side defaults apply.
      for (const [k, v] of Object.entries(p.settings ?? {})) {
        if (v === '' || v == null) delete p.settings[k];
      }
    }
    await invoke('set_config', { config: $state.snapshot(config) });
  }

  // Save & close: the single commit action for the panel.
  async function save() {
    saveError = '';
    try {
      await persist();
      onclose();
    } catch (error) {
      saveError = String(error.message ?? error);
    }
  }

  async function test(id) {
    testResults[id] = { pending: true };
    // Persist any pasted secret/settings first so the test uses them.
    await persist();
    const snap = await invoke('test_provider', { provider: id });
    testResults[id] = snap.error
      ? { ok: false, msg: snap.error.detail }
      : { ok: true, msg: summarize(snap) };
  }

  function summarize(snap) {
    const parts = snap.windows.map((w) => `${w.label} ${w.used_pct.toFixed(0)}%`);
    if (snap.credits) parts.push(`${snap.credits.balance.toFixed(2)} ${snap.credits.unit}`);
    return parts.join(' · ') || 'ok';
  }

  async function clearSecret(id) {
    await invoke('clear_secret', { provider: id });
    secretStored[id] = false;
  }

  function addAccount() {
    const n = Object.entries(config.providers).filter(([id, p]) => (p.kind ?? id) === newKind).length + 1;
    let key = `${newKind}#${n}`;
    while (config.providers[key]) key = `${newKind}#${Number(key.split('#')[1]) + 1}`;
    const info = providerInfo(newKind);
    // Start extra accounts with the same provider configuration as the first
    // configured account of this kind. Credentials remain account-specific.
    const template = Object.entries(config.providers).find(([id, p]) => (p.kind ?? id) === newKind)?.[1];
    config.providers[key] = { kind: newKind, label: newName.trim() || `${info.name} ${n}`, enabled: true, in_tray: true, thresholds: null, alerts: null, low_balance_warn: null, mini_summary_metric: null, mini_summary_metrics: null, tray_metric: null, settings: $state.snapshot(template?.settings ?? {}) };
    ensureFlows();
    // A brand-new account needs configuring, so open it rather than leaving a
    // collapsed row that looks like nothing happened.
    expanded[key] = true;
    newName = '';
    addingAccount = false;
  }

  function removeAccount(id) {
    removalError = '';
    const task = (async () => {
      const kind = config.providers[id].kind ?? id;
      await invoke('clear_secret', { provider: id });
      if (kind === 'claude' || kind === 'codex') await invoke('clear_secret', { provider: `${id}_oauth` });
      delete config.providers[id];
    })();
    pendingRemovals.add(task);
    // Keep the original error for `persist()` to report, without creating an
    // ignored rejected promise from `finally`.
    void task.then(
      () => pendingRemovals.delete(task),
      (error) => {
        pendingRemovals.delete(task);
        removalError = `Could not remove account: ${String(error)}`;
      },
    );
  }

  function moveAccount(id, direction) {
    const entries = Object.entries($state.snapshot(config.providers));
    const index = entries.findIndex(([key]) => key === id);
    const target = index + direction;
    if (index < 0 || target < 0 || target >= entries.length) return;
    const [entry] = entries.splice(index, 1);
    entries.splice(target, 0, entry);
    config.providers = Object.fromEntries(entries);
  }

  // Which account's headline menu is open, so only one is ever up at a time.
  let openMetricMenu = $state('');

  // Per-account disclosure state. Deliberately not persisted to config: with
  // several accounts the panel is mostly scrolling, so every visit starts
  // collapsed and the user opens the one they came for.
  let expanded = $state({});

  function toggleAccount(id) {
    expanded[id] = !expanded[id];
  }

  function toggleMetricMenu(id) {
    openMetricMenu = openMetricMenu === id ? '' : id;
  }

  // The menu is absolutely positioned and outside any form control, so nothing
  // closes it for us.
  function closeMetricMenus(event) {
    if (!event.target.closest?.('.metric-picker')) openMetricMenu = '';
  }

  const selectedMetrics = (account) => account.mini_summary_metrics ?? null;

  function setMetric(account, metricId, checked) {
    const current = selectedMetrics(account) ?? [];
    // Unchecking the last one leaves an empty list, not automatic — otherwise
    // "show nothing for this account" would be unreachable.
    account.mini_summary_metrics = checked
      ? [...current, metricId]
      : current.filter((m) => m !== metricId);
    pruneTrayMetric(account);
  }

  function setAutomatic(account) {
    account.mini_summary_metrics = null;
    pruneTrayMetric(account);
  }

  // A pinned tray metric that is no longer shown would keep driving the icon
  // from off-screen, so drop it back to "worst of selected".
  function pruneTrayMetric(account) {
    const chosen = selectedMetrics(account);
    const pinned = account.tray_metric;
    if (!pinned || pinned === 'none') return;
    if (chosen === null || !chosen.includes(pinned)) account.tray_metric = null;
  }

  function metricSummaryText(id, account) {
    const chosen = selectedMetrics(account);
    if (chosen === null) return 'Automatic';
    if (chosen.length === 0) return 'None';
    const labels = metricOptions(id, account);
    const names = chosen.map((m) => labels.find((o) => o.id === m)?.label ?? m);
    return names.length > 2 ? `${names.length} selected` : names.join(', ');
  }

  // The real metrics an account can show — the known set for its kind, plus
  // anything the live snapshot reports that the known set missed. Automatic
  // and None are states of the selection, not entries here.
  function metricOptions(id, account) {
    const kind = account.kind ?? id;
    const known = {
      claude: [{ id: 'window:five_hour', label: '5-hour' }, { id: 'window:weekly', label: 'Weekly' }],
      codex: [{ id: 'window:weekly', label: 'Weekly' }],
      openrouter: [{ id: 'credits', label: 'Credit balance' }],
      elevenlabs: [{ id: 'window:monthly_credits', label: 'Monthly credits' }],
      hermes: [{ id: 'credits', label: 'Purchased credit balance' }, { id: 'window:monthly_cap', label: 'Monthly cap' }, { id: 'window:monthly_allowance', label: 'Monthly allowance' }],
    }[kind] ?? [];
    const live = snapshots.find((snap) => snap.provider_id === id)?.windows ?? [];
    const choices = [...known];
    for (const window of live) {
      if (window.metric_id) choices.push({ id: `window:${window.metric_id}`, label: window.label });
    }
    // A metric saved before the provider stopped reporting it still needs a row,
    // or unchecking it would be impossible.
    for (const metric of selectedMetrics(account) ?? []) {
      choices.push({ id: metric, label: metric.replace(/^window:/, '') });
    }
    return choices.filter((choice, index, all) => all.findIndex((other) => other.id === choice.id) === index);
  }
</script>

<svelte:window onclick={closeMetricMenus} />

<div class="settings">
    <section>
      <h2>Providers</h2>
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
        {@const p = providerInfo(account.kind ?? id)}
        <div class="provider">
          <div class="provider-header row" class:collapsed={!expanded[id]}>
            <button
              class="provider-disclosure"
              aria-expanded={expanded[id] === true}
              onclick={() => toggleAccount(id)}
            ><span class="chevron" class:open={expanded[id]}>▸</span> <strong>{account.label ?? p.name}</strong></button>
            <span class="spacer"></span>
            <label class="inline">
              <input type="checkbox" bind:checked={account.enabled} />
              Enabled
            </label>
            <button class="small" title="Move account up" aria-label={`Move ${account.label ?? p.name} up`} disabled={index === 0} onclick={() => moveAccount(id, -1)}>↑</button>
            <button class="small" title="Move account down" aria-label={`Move ${account.label ?? p.name} down`} disabled={index === Object.keys(config.providers).length - 1} onclick={() => moveAccount(id, 1)}>↓</button>
          </div>
          {#if expanded[id]}
          <p class="note">{p.note}</p>
          <label class="field">Account name <input maxlength="40" bind:value={account.label} placeholder={p.name} /></label>
          <div class="field metric-picker">
            <span>Mini-summary headlines</span>
            <button
              class="metric-toggle"
              aria-expanded={openMetricMenu === id}
              onclick={() => toggleMetricMenu(id)}
            >{metricSummaryText(id, account)} ▾</button>
            {#if openMetricMenu === id}
              <div class="metric-menu">
                <!-- Pinned above the metrics and mutually exclusive with them:
                     automatic is a mode, not one more headline. -->
                <label class="metric-item">
                  <input type="checkbox" checked={selectedMetrics(account) === null} onchange={() => setAutomatic(account)} />
                  Automatic
                </label>
                <hr />
                {#each metricOptions(id, account) as metric}
                  <label class="metric-item">
                    <input
                      type="checkbox"
                      checked={selectedMetrics(account)?.includes(metric.id) ?? false}
                      onchange={(event) => setMetric(account, metric.id, event.currentTarget.checked)}
                    />
                    {metric.label}
                  </label>
                {/each}
              </div>
            {/if}
          </div>
          {#if account.enabled}
            <label class="field">Tray icon status
              <select value={account.tray_metric ?? ''} onchange={(event) => (account.tray_metric = event.currentTarget.value || null)}>
                <option value="">Worst of selected</option>
                <option value="none">None</option>
                {#each metricOptions(id, account).filter((m) => selectedMetrics(account)?.includes(m.id)) as metric}
                  <option value={metric.id}>{metric.label}</option>
                {/each}
              </select>
            </label>
          {/if}
          {#if p.secret}
            <div class="row">
              <input
                type="password"
                placeholder={secretStored[id] ? `${p.secret} stored — paste to replace` : `Paste ${p.secret}`}
                bind:value={secretInputs[id]}
              />
              {#if secretStored[id]}
                <button class="small" onclick={() => clearSecret(id)}>Clear</button>
              {/if}
            </div>
          {/if}
          {#if p.id === 'claude'}
            {@const claudeFlow = oauthView(id)}
            <label class="field">Sign-in method
              <select bind:value={account.settings.auth_mode}>
                <option value={undefined}>Auto (CLI, then built-in)</option>
                <option value="cli">Claude Code CLI only</option>
                <option value="oauth">Built-in sign-in only</option>
              </select>
            </label>
            {#if account.settings.auth_mode !== 'cli'}
              <div class="row">
                {#if claudeFlow.signedIn}
                  <span class="test good">Built-in sign-in active ✓</span>
                  <button class="small" onclick={() => oauthClear(id)}>Sign out</button>
                {:else}
                  <button class="small" onclick={() => oauthStart(id)}>Sign in with Claude…</button>
                {/if}
              </div>
              {#if claudeFlow.url}
                <p class="note">
                  A browser window opened (or open this link yourself):
                  <span class="wrap">{claudeFlow.url}</span><br />
                  Authorize, then paste the code shown:
                </p>
                <div class="row">
                  <input type="text" placeholder="Paste code (looks like abc123#xyz789)" bind:value={claudeFlow.code} />
                  <button class="small" onclick={() => oauthFinish(id)}>Finish</button>
                </div>
              {/if}
              {#if claudeFlow.status && claudeFlow.status !== 'ok'}
                <p class="test bad">{claudeFlow.status}</p>
              {/if}
            {/if}
          {/if}
          {#if p.id === 'codex'}
            {@const codexFlow = codexView(id)}
            <label class="field">Sign-in method
              <select bind:value={account.settings.auth_mode}>
                <option value={undefined}>Auto (CLI, then built-in)</option>
                <option value="cli">Codex CLI only</option>
                <option value="oauth">Built-in sign-in only</option>
              </select>
            </label>
            {#if account.settings.auth_mode !== 'cli'}
              <div class="row">
                {#if codexFlow.signedIn}
                  <span class="test good">Built-in sign-in active ✓</span>
                  <button class="small" onclick={() => codexClear(id)}>Sign out</button>
                {:else}
                  <button class="small" onclick={() => codexStart(id)}>Sign in with Codex…</button>
                {/if}
              </div>
              {#if codexFlow.userCode}
                <p class="note">
                  A browser window opened (or open this link yourself):
                  <span class="wrap">{codexFlow.url}</span><br />
                  Enter this code to authorize:
                </p>
                <p class="device-code">{codexFlow.userCode}</p>
                <p class="note">Waiting for authorization… (the code expires after 15 minutes)</p>
              {/if}
              {#if codexFlow.status && codexFlow.status !== 'waiting'}
                <p class="test bad">{codexFlow.status}</p>
              {/if}
            {/if}
          {/if}
          {#if p.id === 'hermes'}
            <label class="field">Source
              <select bind:value={account.settings.source}>
                <option value={undefined}>Auto (local → SSH → cookie)</option>
                <option value="hermes">Local hermes-agent</option>
                <option value="remote">Remote over SSH</option>
                <option value="cookie">Session cookie</option>
              </select>
            </label>
            {#if account.settings.source !== 'cookie' && account.settings.source !== 'hermes'}
              <label class="field">Remote SSH host
                <input
                  type="text"
                  placeholder="user@server (key auth)"
                  bind:value={account.settings.ssh_host}
                />
              </label>
              <label class="field">Transport
                <select bind:value={account.settings.transport}>
                  <option value={undefined}>Plain SSH</option>
                  <option value="tailscale">Tailscale SSH</option>
                </select>
              </label>
            {/if}
            {#if account.settings.source === 'cookie'}
              <label class="field">Balance endpoint
                <input
                  type="text"
                  placeholder="default: portal billing API"
                  bind:value={account.settings.endpoint}
                />
              </label>
            {/if}
            <label class="field">Price per token (optional)
              <input
                type="number"
                step="any"
                placeholder="for tokens-left estimate"
                bind:value={account.settings.token_price}
              />
            </label>
          {/if}
          {#if p.id === 'openrouter' || p.id === 'hermes'}
            <div class="row">
              <label class="inline">Low-balance warning at
                <input type="number" step="any" class="num" bind:value={account.low_balance_warn} placeholder="off" />
              </label>
            </div>
          {/if}
          {#if testResults[id]}
            <p class="test {testResults[id].ok ? 'good' : 'bad'}">
              {testResults[id].pending ? 'testing…' : testResults[id].msg}
            </p>
          {/if}
          <!-- Test sits with the credential controls it exercises, not up in
               the header: it only makes sense once an account is open. -->
          <div class="provider-footer">
            <button class="small" onclick={() => test(id)}>Test</button>
            <span class="spacer"></span>
            <button class="small" onclick={() => removeAccount(id)}>Remove account</button>
          </div>
          {/if}
          </div>
      {/each}
    </section>

    <section>
      <h2>Thresholds</h2>
      <div class="row">
        <label class="inline">Warn at <input type="number" class="num" bind:value={config.thresholds.warn_pct} />%</label>
        <label class="inline">Critical at <input type="number" class="num" bind:value={config.thresholds.critical_pct} />%</label>
      </div>
    </section>

    <section>
      <h2>Alerts</h2>
      <label class="row"><input type="checkbox" bind:checked={config.alerts.toast} /> Toast notification</label>
      <label class="row"><input type="checkbox" bind:checked={config.alerts.tray_color} /> Tray icon color</label>
      <label class="row"><input type="checkbox" bind:checked={config.alerts.auto_popup} /> Auto-popup window</label>
    </section>

    <section>
      <h2>General</h2>
      <div class="row">
        <label class="inline">Poll every <input type="number" class="num" bind:value={config.poll_interval_secs} /> s</label>
      </div>
      <label class="row"><input type="checkbox" bind:checked={config.autostart} /> Start on login</label>
      <label class="row"><input type="checkbox" bind:checked={config.hide_on_blur} /> Hide when clicking outside</label>
      <div class="row">
        <label class="inline">Order accounts by
          <select bind:value={config.sort_order}>
            {#each SORT_ORDERS as order}<option value={order.id}>{order.label}</option>{/each}
          </select>
        </label>
      </div>
      <!-- The basis chooses which number sorts, so it means nothing while the
           order is the user's own. Disabled rather than hidden: it keeps its
           saved value, and the row does not jump as the order changes. -->
      <div class="row">
        <label class="inline" class:disabled={config.sort_order === 'manual'}>Sorting on
          <select bind:value={config.sort_basis} disabled={config.sort_order === 'manual'}>
            {#each SORT_BASES as basis}<option value={basis.id}>{basis.label}</option>{/each}
          </select>
        </label>
      </div>
      <p class="note">Ordering applies to the main window, the mini summary, and the tray tooltip alike. Accounts with no matching number — a credits-only balance, or an account that isn't in the tray — stay at the bottom in your own order.</p>
      <label class="row"><input type="checkbox" bind:checked={config.mini_summary_bars} /> Show usage bars in the mini summary</label>
      <label class="row"><input type="checkbox" bind:checked={config.scroll_opacity} /> Fade windows when scrolling over them</label>
      <p class="note">Esc, ✕, and the tray icon always hide the widget. This extra click-away dismiss can occasionally fight window dragging.</p>
      {#if onWayland}
        <p class="note">
          You're on Wayland, which has no always-on-top protocol — the widget
          will slip behind other windows when they take focus, whatever the
          setting above says. Launching it with <code>GDK_BACKEND=x11</code>
          restores it.
        </p>
      {/if}
    </section>

    <div class="actions">
      <button class="primary" onclick={save}>Save &amp; close</button>
    </div>
    {#if saveError}
      <p class="test bad">{saveError}</p>
    {/if}
    {#if appVersion}
      <p class="version">Quota Widget v{appVersion}</p>
    {/if}
  </div>
