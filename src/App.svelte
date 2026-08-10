<script>
  import { onMount, tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { LogicalSize } from '@tauri-apps/api/dpi';
  import ProviderCard from './lib/ProviderCard.svelte';
  import Settings from './lib/Settings.svelte';
  import { resetOpacity, stepOpacity } from './lib/opacity.js';

  const APP_VERSION = __QUOTA_WIDGET_VERSION__;
  const BUILD_BRANCH = __QUOTA_WIDGET_BRANCH__;
  let view = $state('popup'); // 'popup' | 'settings'
  let snapshots = $state([]);
  let appConfig = $state(null);
  let refreshing = $state(false);
  let headerEl = $state(null);
  let cardsEl = $state(null);

  const SETTINGS_HEIGHT = 560;

  // First-run desktop integration, AppImage only. Shown once: whichever way it
  // is answered, Rust records that the question was put, so "Not now" is
  // remembered as firmly as yes and no launch nags again. Settings keeps the
  // explicit add and remove controls, so a deferral is never final.
  let integrationPrompt = $state(null);
  let integrationResult = $state('');

  async function maybeAskAboutIntegration() {
    // Only a running AppImage answers with a status, and only an unclaimed
    // launcher is worth asking about — a repair is a Settings decision, not a
    // question to open with.
    if (appConfig?.desktop_integration_prompted) return;
    try {
      const status = await invoke('desktop_integration_status');
      if (status?.state === 'absent') integrationPrompt = status;
    } catch {
      // No such command in this shell: nothing to integrate.
    }
  }

  async function answerIntegration(add) {
    integrationPrompt = null;
    try {
      if (add) {
        await invoke('desktop_integration_add');
        integrationResult = 'Added to your applications menu.';
      }
    } catch (error) {
      integrationResult = `Could not add the launcher: ${error}`;
    }
    // Recorded whichever way it was answered, and even if the write failed:
    // the user has been asked, and re-asking at every launch is the nag this
    // exists to prevent. Settings still offers the action.
    await invoke('mark_desktop_integration_prompted').catch(() => {});
  }

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
    resetOpacity();
    invoke('get_snapshots').then((initial) => {
      snapshots = initial.snapshots;
      appConfig = initial.config;
      void maybeAskAboutIntegration();
    });
    const unlisten = [];
    listen('snapshots', (e) => (snapshots = e.payload)).then((u) => unlisten.push(u));
    // Settings saves through Rust, which broadcasts the canonical persisted
    // config. Keep the next Settings visit in sync without replacing the
    // active component's local draft while it is being edited.
    listen('config', (e) => {
      appConfig = e.payload;
      if (!e.payload.scroll_opacity) resetOpacity();
    }).then((u) => unlisten.push(u));
    listen('navigate', (e) => (view = e.payload)).then((u) => unlisten.push(u));
    // Hiding to tray doesn't unload the page, so `view` would otherwise
    // persist: reopening after a visit to Settings would land back in
    // Settings instead of the usage list. Rust emits this on every show.
    listen('window-shown', () => {
      view = 'popup';
      resetOpacity();
    }).then((u) => unlisten.push(u));
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

  function fadeOnWheel(event) {
    if (!appConfig?.scroll_opacity) return;
    // The popup's actual content is scrollable, so only its chrome fades.
    // Let the cards and the Settings form keep their normal wheel behaviour.
    if (event.target.closest('.cards, .settings')) return;
    if (stepOpacity(event.deltaY, 0.15, appConfig.scroll_opacity_invert)) event.preventDefault();
  }

</script>

<main onwheel={fadeOnWheel}>
  <header role="toolbar" aria-label="Window controls" tabindex="-1" data-tauri-drag-region bind:this={headerEl} onmousedown={() => invoke('note_drag')}>
    <span class="title" data-tauri-drag-region>Quota Widget <small class="build-version" data-tauri-drag-region>v{APP_VERSION}</small>{#if BUILD_BRANCH} <small class="build-branch" data-tauri-drag-region>{BUILD_BRANCH}</small>{/if}</span>
    <span class="spacer" data-tauri-drag-region></span>
    {#if view === 'popup'}
      <button class="icon" title="Refresh now" class:spin={refreshing} onclick={refresh}>⟳</button>
      <button class="icon" title="Settings" onclick={() => (view = 'settings')}>⚙</button>
    {:else}
      <button class="icon" title="Back" onclick={() => (view = 'popup')}>←</button>
    {/if}
    <button class="icon" title="Hide to tray" onclick={() => invoke('hide_window')}>✕</button>
  </header>

  <!-- Consent before anything is written outside the config dir. It sits above
       the cards rather than in a modal so it can be ignored: it disappears on
       either answer, and Settings keeps both controls afterwards. -->
  {#if integrationPrompt}
    <div class="integration-prompt">
      <p>Add Quota Widget to your applications menu? This writes a launcher and icons under your home directory only.</p>
      <div class="integration-actions">
        <button class="link" onclick={() => answerIntegration(false)}>Not now</button>
        <button onclick={() => answerIntegration(true)}>Add it</button>
      </div>
    </div>
  {:else if integrationResult}
    <p class="integration-result">{integrationResult}</p>
  {/if}

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
      <Settings initialConfig={appConfig} {snapshots} onclose={() => (view = 'popup')} />
    {:else}
      <p class="empty">Loading…</p>
    {/if}
  {/if}
</main>
