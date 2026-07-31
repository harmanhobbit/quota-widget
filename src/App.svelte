<script>
  import { onMount } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import ProviderCard from './lib/ProviderCard.svelte';
  import Settings from './lib/Settings.svelte';

  let view = $state('popup'); // 'popup' | 'settings'
  let snapshots = $state([]);
  let refreshing = $state(false);

  onMount(() => {
    invoke('get_snapshots').then((s) => (snapshots = s));
    const unlisten = [];
    listen('snapshots', (e) => (snapshots = e.payload)).then((u) => unlisten.push(u));
    listen('navigate', (e) => (view = e.payload)).then((u) => unlisten.push(u));
    const esc = (e) => {
      if (e.key === 'Escape') {
        if (view === 'settings') view = 'popup';
        else invoke('hide_window');
      }
    };
    window.addEventListener('keydown', esc);
    return () => {
      unlisten.forEach((u) => u());
      window.removeEventListener('keydown', esc);
    };
  });

  async function refresh() {
    refreshing = true;
    await invoke('refresh_now');
    setTimeout(() => (refreshing = false), 1200);
  }
</script>

<main>
  <header data-tauri-drag-region>
    <span class="title" data-tauri-drag-region>Quota Widget</span>
    <span class="spacer" data-tauri-drag-region></span>
    {#if view === 'popup'}
      <button class="icon" title="Refresh now" class:spin={refreshing} onclick={refresh}>⟳</button>
      <button class="icon" title="Settings" onclick={() => (view = 'settings')}>⚙</button>
    {:else}
      <button class="icon" title="Back" onclick={() => (view = 'popup')}>←</button>
    {/if}
    <button class="icon" title="Hide to tray" onclick={() => invoke('hide_window')}>✕</button>
  </header>

  {#if view === 'popup'}
    <div class="cards">
      {#if snapshots.length === 0}
        <p class="empty">
          No providers enabled yet — open <button class="link" onclick={() => (view = 'settings')}>Settings</button> to add one.
        </p>
      {:else}
        {#each snapshots as snap (snap.provider_id)}
          <ProviderCard {snap} />
        {/each}
      {/if}
    </div>
  {:else}
    <Settings onclose={() => (view = 'popup')} />
  {/if}
</main>
