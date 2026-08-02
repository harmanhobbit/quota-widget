<script>
  import { onMount, tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { LogicalSize } from '@tauri-apps/api/dpi';
  import ProviderCard from './lib/ProviderCard.svelte';
  import Settings from './lib/Settings.svelte';

  const APP_VERSION = __QUOTA_WIDGET_VERSION__;
  let view = $state('popup'); // 'popup' | 'settings'
  let snapshots = $state([]);
  let appConfig = $state(null);
  let refreshing = $state(false);
  let headerEl = $state(null);
  let cardsEl = $state(null);

  const SETTINGS_HEIGHT = 560;

  // The popup shrinks to exactly fit the usage meters; settings gets a fixed
  // comfortable height (still user-resizable from there).
  async function fitToContent() {
    try {
      const win = getCurrentWindow();
      if (view === 'popup' && headerEl && cardsEl) {
        const h = Math.min(680, Math.max(120, headerEl.offsetHeight + cardsEl.scrollHeight + 2));
        await win.setSize(new LogicalSize(window.innerWidth, h));
      } else if (view === 'settings') {
        await win.setSize(new LogicalSize(window.innerWidth, SETTINGS_HEIGHT));
      }
    } catch {
      // sizing is cosmetic — never let it break the UI
    }
  }

  $effect(() => {
    void view;
    void snapshots;
    tick().then(fitToContent);
  });

  onMount(() => {
    invoke('get_snapshots').then((initial) => {
      snapshots = initial.snapshots;
      appConfig = initial.config;
    });
    const unlisten = [];
    listen('snapshots', (e) => (snapshots = e.payload)).then((u) => unlisten.push(u));
    listen('navigate', (e) => (view = e.payload)).then((u) => unlisten.push(u));
    // Hiding to tray doesn't unload the page, so `view` would otherwise
    // persist: reopening after a visit to Settings would land back in
    // Settings instead of the usage list. Rust emits this on every show.
    listen('window-shown', () => (view = 'popup')).then((u) => unlisten.push(u));
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
  <header role="toolbar" aria-label="Window controls" tabindex="-1" data-tauri-drag-region bind:this={headerEl} onmousedown={() => invoke('note_drag')}>
    <span class="title" data-tauri-drag-region>Quota Widget <small class="build-version" data-tauri-drag-region>v{APP_VERSION}</small></span>
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
    <div class="cards" bind:this={cardsEl}>
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
    {#if appConfig}
      <Settings initialConfig={appConfig} onclose={() => (view = 'popup')} />
    {:else}
      <p class="empty">Loading…</p>
    {/if}
  {/if}
</main>
