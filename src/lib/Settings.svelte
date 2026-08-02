<script>
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';

  let { onclose, initialConfig } = $props();

  const PROVIDERS = [
    { id: 'claude', name: 'Claude', secret: null, note: 'Uses the Claude Code CLI login if present, or the built-in browser sign-in below.' },
    { id: 'codex', name: 'Codex', secret: null, note: 'Uses the Codex CLI login if present, or the built-in device sign-in below.' },
    { id: 'openrouter', name: 'OpenRouter', secret: 'API key', note: 'Create a key at openrouter.ai/keys.' },
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

  async function initialiseSettings() {
    // Normalize configured accounts only. Do not recreate removed defaults.
    for (const account of Object.values(config.providers)) {
      account.settings ??= {};
      account.in_tray ??= true;
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
    return () => unlisten?.();
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
    await persist();
    onclose();
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
    const n = Object.values(config.providers).filter((p) => (p.kind ?? '') === newKind).length + 1;
    let key = `${newKind}#${n}`;
    while (config.providers[key]) key = `${newKind}#${Number(key.split('#')[1]) + 1}`;
    const info = providerInfo(newKind);
    // Start extra accounts with the same provider configuration as the first
    // configured account of this kind. Credentials remain account-specific.
    const template = Object.entries(config.providers).find(([id, p]) => (p.kind ?? id) === newKind)?.[1];
    config.providers[key] = { kind: newKind, label: newName.trim() || `${info.name} ${n}`, enabled: true, in_tray: true, thresholds: null, alerts: null, low_balance_warn: null, settings: $state.snapshot(template?.settings ?? {}) };
    ensureFlows();
    newName = '';
  }

  async function removeAccount(id) {
    const kind = config.providers[id].kind ?? id;
    await invoke('clear_secret', { provider: id });
    if (kind === 'claude' || kind === 'codex') await invoke('clear_secret', { provider: `${id}_oauth` });
    delete config.providers[id];
  }
</script>

<div class="settings">
    <section>
      <h2>Providers</h2>
      <div class="row"><select bind:value={newKind}>{#each PROVIDERS as p}<option value={p.id}>{p.name}</option>{/each}</select><input placeholder="Account name (optional)" bind:value={newName} /><button class="small" onclick={addAccount}>Add account</button></div>
      {#each Object.entries(config.providers) as [id, account] (id)}
        {@const p = providerInfo(account.kind ?? id)}
        <div class="provider">
          <label class="row">
            <input type="checkbox" bind:checked={account.enabled} />
            <strong>{account.label ?? p.name}</strong>
            <span class="spacer"></span>
            <button class="small" onclick={() => test(id)}>Test</button>
          </label>
          <p class="note">{p.note}</p>
          <label class="field">Account name <input maxlength="40" bind:value={account.label} placeholder={p.name} /></label>
          {#if account.enabled}
            <label class="row sub-toggle">
              <input type="checkbox" bind:checked={account.in_tray} />
              Include in tray icon
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
          </div>
          <button class="small" onclick={() => removeAccount(id)}>Remove account</button>
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
      <label class="row"><input type="checkbox" bind:checked={config.mini_summary_bars} /> Show usage bars in the mini summary</label>
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
    {#if appVersion}
      <p class="version">Quota Widget v{appVersion}</p>
    {/if}
  </div>
