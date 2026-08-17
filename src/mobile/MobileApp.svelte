<script>
  // Mobile shell: opens directly to the usage list, no window/tray concepts.
  // Proves the foreground path for one direct-HTTPS pasted-key provider
  // (OpenRouter for this ticket — see issue #108) driven entirely through the
  // same quota-core refresh operation and IPC commands desktop uses, via
  // `../lib/host.js`.
  import { onMount } from 'svelte';
  import UsageCard from '../lib/shared/UsageCard.svelte';
  import Ordering from '../lib/shared/Ordering.svelte';
  import Thresholds from '../lib/shared/Thresholds.svelte';
  import SimpleKeyAccount from '../lib/shared/SimpleKeyAccount.svelte';
  import { getSnapshots, setConfig, setSecret, hasSecret, clearSecret, refreshNow, testProvider, listen } from '../lib/host.js';
  import { invoke } from '@tauri-apps/api/core';

  let view = $state('list'); // 'list' | 'settings'
  let snapshots = $state([]);
  let config = $state(null);
  let refreshing = $state(false);
  let secretStored = $state(false);
  let secretInput = $state('');
  let testResult = $state(null);
  let headlineOpen = $state(false);
  let accountExpanded = $state(true);

  const HEADLINE_OPTIONS = [
    { id: 'credits', label: 'Credit balance' },
    { id: 'window:monthly_spend', label: 'Monthly' },
  ];

  // The live snapshot may report a metric the static list above missed.
  const headlineOptions = $derived.by(() => {
    const live = snapshots.find((s) => s.provider_id === 'openrouter')?.windows ?? [];
    const extra = live
      .filter((w) => w.metric_id)
      .map((w) => ({ id: `window:${w.metric_id}`, label: w.label }))
      .filter((choice) => !HEADLINE_OPTIONS.some((o) => o.id === choice.id));
    return [...HEADLINE_OPTIONS, ...extra];
  });

  function newOpenRouterAccount() {
    return {
      kind: 'openrouter',
      label: null,
      enabled: true,
      in_tray: true,
      thresholds: null,
      alerts: null,
      low_balance_warn: null,
      mini_summary_metric: null,
      mini_summary_metrics: null,
      tray_metric: null,
      settings: {},
    };
  }

  async function addAccount() {
    config.providers.openrouter = newOpenRouterAccount();
    await persist();
  }

  async function persist() {
    if (secretInput) {
      await setSecret('openrouter', secretInput);
      secretStored = true;
      secretInput = '';
    }
    await setConfig($state.snapshot(config));
  }

  async function test() {
    testResult = { pending: true };
    await persist();
    const snap = await testProvider('openrouter');
    testResult = snap.error
      ? { ok: false, msg: snap.error.detail }
      : { ok: true, msg: 'ok' };
  }

  async function removeAccount() {
    await clearSecret('openrouter');
    delete config.providers.openrouter;
    secretStored = false;
    await persist();
  }

  async function refresh() {
    refreshing = true;
    await refreshNow();
    setTimeout(() => (refreshing = false), 1200);
  }

  onMount(() => {
    getSnapshots().then(async (initial) => {
      snapshots = initial.snapshots;
      config = initial.config;
      secretStored = await hasSecret('openrouter').catch(() => false);
      // Debug-only CI seed path (see .github/workflows/build.yml, the
      // `android` job): compiled entirely out of release builds. When set,
      // it lets CI prove live quota renders without fragile UI-automation
      // typing.
      try {
        const ciKey = await invoke('ci_test_key');
        if (ciKey && !config.providers.openrouter) {
          config.providers.openrouter = newOpenRouterAccount();
          secretInput = ciKey;
          await persist();
          await refresh();
        }
      } catch {
        // No such command outside a debug build — nothing to seed.
      }
    });
    const unlisten = [];
    listen('snapshots', (e) => (snapshots = e.payload)).then((u) => unlisten.push(u));
    listen('config', (e) => (config = e.payload)).then((u) => unlisten.push(u));
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
          <UsageCard {snap} />
        {/each}
      {/if}
    </div>
  {:else}
    <div class="settings mobile-settings">
      <section>
        <h2>OpenRouter</h2>
        {#if config.providers.openrouter}
          <SimpleKeyAccount
            id="openrouter"
            bind:account={config.providers.openrouter}
            providerName="OpenRouter"
            providerNote="Create a key at openrouter.ai/keys. Optional monthly budget tracks this month's spend against your target."
            {secretStored}
            bind:secretInput
            lowBalanceWarn={true}
            {headlineOptions}
            headlineOpen={headlineOpen}
            onToggleHeadline={() => (headlineOpen = !headlineOpen)}
            bind:expanded={accountExpanded}
            {testResult}
            onTest={test}
            onClearSecret={async () => { await clearSecret('openrouter'); secretStored = false; }}
            onRemove={removeAccount}
          />
        {:else}
          <button onclick={addAccount}>Add OpenRouter account</button>
        {/if}
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
