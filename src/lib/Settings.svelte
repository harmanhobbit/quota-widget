<script>
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';

  let { onclose } = $props();

  const PROVIDERS = [
    { id: 'claude', name: 'Claude', secret: null, note: 'Uses the Claude Code login on this machine (run `claude` once to sign in).' },
    { id: 'codex', name: 'Codex', secret: null, note: 'Uses the Codex CLI login on this machine (run `codex` once to sign in).' },
    { id: 'openrouter', name: 'OpenRouter', secret: 'API key', note: 'Create a key at openrouter.ai/keys.' },
    { id: 'hermes', name: 'Hermes Portal', secret: 'Session cookie', note: 'Unofficial: paste the Cookie header from a logged-in portal.nousresearch.com request (browser DevTools → Network). May break if the portal changes.' },
  ];

  let config = $state(null);
  let secretInputs = $state({});
  let secretStored = $state({});
  let testResults = $state({});
  let saved = $state(false);

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
  });

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
    </section>

    <div class="actions">
      <button class="primary" onclick={save}>{saved ? 'Saved ✓' : 'Save'}</button>
      <button onclick={onclose}>Done</button>
    </div>
  </div>
{:else}
  <p class="empty">Loading…</p>
{/if}
