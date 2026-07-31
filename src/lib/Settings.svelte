<script>
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';

  let { onclose } = $props();

  const PROVIDERS = [
    { id: 'claude', name: 'Claude', secret: null, note: 'Uses the Claude Code CLI login if present, or the built-in browser sign-in below.' },
    { id: 'codex', name: 'Codex', secret: null, note: 'Uses the Codex CLI login on this machine (run `codex` once to sign in).' },
    { id: 'openrouter', name: 'OpenRouter', secret: 'API key', note: 'Create a key at openrouter.ai/keys.' },
    { id: 'hermes', name: 'Hermes Portal', secret: 'Session cookie', note: 'Auto-detects a hermes-agent login (~/.hermes/auth.json) and uses the portal billing API. Cookie paste below is only a fallback for machines without hermes-agent.' },
  ];

  let config = $state(null);
  let secretInputs = $state({});
  let secretStored = $state({});
  let testResults = $state({});
  let saved = $state(false);
  let oauth = $state({ url: '', code: '', status: '', signedIn: false });

  onMount(async () => {
    const cfg = await invoke('get_config');
    // Normalize so every provider entry exists and is directly bindable.
    for (const p of PROVIDERS) {
      cfg.providers[p.id] ??= { enabled: false, thresholds: null, alerts: null, low_balance_warn: null, settings: {} };
      cfg.providers[p.id].settings ??= {};
    }
    config = cfg;
    for (const p of PROVIDERS) {
      if (p.secret) secretStored[p.id] = await invoke('has_secret', { provider: p.id });
    }
    oauth.signedIn = await invoke('has_secret', { provider: 'claude_oauth' });
  });

  async function oauthStart() {
    oauth.status = '';
    oauth.url = await invoke('claude_oauth_start');
  }

  async function oauthFinish() {
    try {
      await invoke('claude_oauth_finish', { code: oauth.code });
      oauth.status = 'ok';
      oauth.signedIn = true;
      oauth.url = '';
      oauth.code = '';
    } catch (e) {
      oauth.status = String(e);
    }
  }

  async function oauthClear() {
    await invoke('clear_secret', { provider: 'claude_oauth' });
    oauth.signedIn = false;
  }

  async function save() {
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
    saved = true;
    setTimeout(() => (saved = false), 1500);
  }

  async function test(id) {
    testResults[id] = { pending: true };
    // Persist any pasted secret/settings first so the test uses them.
    await save();
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
</script>

{#if config}
  <div class="settings">
    <section>
      <h2>Providers</h2>
      {#each PROVIDERS as p (p.id)}
        <div class="provider">
          <label class="row">
            <input type="checkbox" bind:checked={config.providers[p.id].enabled} />
            <strong>{p.name}</strong>
            <span class="spacer"></span>
            <button class="small" onclick={() => test(p.id)}>Test</button>
          </label>
          <p class="note">{p.note}</p>
          {#if p.secret}
            <div class="row">
              <input
                type="password"
                placeholder={secretStored[p.id] ? `${p.secret} stored — paste to replace` : `Paste ${p.secret}`}
                bind:value={secretInputs[p.id]}
              />
              {#if secretStored[p.id]}
                <button class="small" onclick={() => clearSecret(p.id)}>Clear</button>
              {/if}
            </div>
          {/if}
          {#if p.id === 'claude'}
            <div class="row">
              <label class="inline">Sign-in method
                <select bind:value={config.providers['claude'].settings.auth_mode}>
                  <option value={undefined}>Auto (CLI, then built-in)</option>
                  <option value="cli">Claude Code CLI only</option>
                  <option value="oauth">Built-in sign-in only</option>
                </select>
              </label>
            </div>
            {#if config.providers['claude'].settings.auth_mode !== 'cli'}
              <div class="row">
                {#if oauth.signedIn}
                  <span class="test good">Built-in sign-in active ✓</span>
                  <button class="small" onclick={oauthClear}>Sign out</button>
                {:else}
                  <button class="small" onclick={oauthStart}>Sign in with Claude…</button>
                {/if}
              </div>
              {#if oauth.url}
                <p class="note">
                  A browser window opened (or open this link yourself):
                  <span class="wrap">{oauth.url}</span><br />
                  Authorize, then paste the code shown:
                </p>
                <div class="row">
                  <input type="text" placeholder="Paste code (looks like abc123#xyz789)" bind:value={oauth.code} />
                  <button class="small" onclick={oauthFinish}>Finish</button>
                </div>
              {/if}
              {#if oauth.status && oauth.status !== 'ok'}
                <p class="test bad">{oauth.status}</p>
              {/if}
            {/if}
          {/if}
          {#if p.id === 'hermes'}
            <div class="row">
              <input
                type="text"
                placeholder="Balance endpoint URL (from DevTools)"
                bind:value={config.providers['hermes'].settings.endpoint}
              />
            </div>
            <div class="row">
              <input
                type="number"
                step="any"
                placeholder="Price per token (optional, for tokens-left estimate)"
                bind:value={config.providers['hermes'].settings.token_price}
              />
            </div>
          {/if}
          {#if p.id === 'openrouter' || p.id === 'hermes'}
            <div class="row">
              <label class="inline">Low-balance warning at
                <input type="number" step="any" class="num" bind:value={config.providers[p.id].low_balance_warn} placeholder="off" />
              </label>
            </div>
          {/if}
          {#if testResults[p.id]}
            <p class="test {testResults[p.id].ok ? 'good' : 'bad'}">
              {testResults[p.id].pending ? 'testing…' : testResults[p.id].msg}
            </p>
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
      <label class="row"><input type="checkbox" bind:checked={config.autostart} /> Start with Windows</label>
      <label class="row"><input type="checkbox" bind:checked={config.hide_on_blur} /> Hide when clicking outside</label>
      <p class="note">Esc, ✕, and the tray icon always hide the widget. This extra click-away dismiss can occasionally fight window dragging.</p>
    </section>

    <div class="actions">
      <button class="primary" onclick={save}>{saved ? 'Saved ✓' : 'Save'}</button>
      <button onclick={onclose}>Done</button>
    </div>
  </div>
{:else}
  <p class="empty">Loading…</p>
{/if}
