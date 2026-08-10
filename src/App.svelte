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
  // The Settings return state: where a Settings visit goes when it exits
  // through Save & close, Back, or Escape. 'popup' | 'mini' | 'hidden'.
  // Captured per visit and never persisted — it describes what was on screen a
  // moment ago, which is not a preference the user set. Settings opened from
  // the usage popup always returns there; the tray decides the other two and
  // sends its answer with the navigation, since only Rust can see that a mini
  // summary was up before the full window covered it.
  let settingsReturn = $state('popup');
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
    resetOpacity();
    invoke('get_snapshots').then((initial) => {
      snapshots = initial.snapshots;
      appConfig = initial.config;
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
    listen('navigate', (e) => {
      view = e.payload.view;
      settingsReturn = e.payload.return_to ?? 'popup';
    }).then((u) => unlisten.push(u));
    // Hiding to tray doesn't unload the page, so `view` would otherwise
    // persist: reopening after a visit to Settings would land back in
    // Settings instead of the usage list. Rust emits this on every show.
    // The captured return state goes with it — it belongs to the visit that
    // just ended, and the next visit captures its own.
    listen('window-shown', () => {
      view = 'popup';
      settingsReturn = 'popup';
      resetOpacity();
    }).then((u) => unlisten.push(u));
    const esc = (e) => {
      if (e.key === 'Escape') {
        // Escape out of Settings discards the unsaved draft — the component is
        // unmounted with it — and takes the same exit as Save & close.
        if (view === 'settings') leaveSettings();
        else invoke('hide_window');
      }
    };
    window.addEventListener('keydown', esc);
    return () => {
      unlisten.forEach((u) => u());
      window.removeEventListener('keydown', esc);
    };
  });

  // Settings entered from the usage popup: the popup is on screen and is where
  // this visit returns to, so no shell involvement is needed either way.
  function openSettings() {
    settingsReturn = 'popup';
    view = 'settings';
  }

  // Back is navigation *within* this window, not an exit: it swaps the view to
  // the usage list and leaves the window on screen, whatever the visit was
  // entered from. Unsaved edits go with the unmounted component, as they always
  // did — what changed is where the user ends up.
  //
  // The capture is deliberately left alone rather than cleared here: every way
  // back into Settings sets it first — the gear through `openSettings`, the
  // tray through `navigate` — so a stale value is unreachable, and clearing it
  // would be a write no test could observe.
  function backToUsage() {
    view = 'popup';
  }

  // Leaving Settings — Save & close, or Escape — lands on the captured return
  // state. The usage popup is the frontend's own to restore; the other two are
  // window lifecycle, which only Rust can drive. A failed save never reaches
  // here, so Settings stays mounted with its error and its return state intact.
  function leaveSettings() {
    const to = settingsReturn;
    view = 'popup';
    settingsReturn = 'popup';
    if (to !== 'popup') invoke('exit_settings', { to });
  }

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
      <button class="icon" title="Settings" onclick={openSettings}>⚙</button>
    {:else}
      <button class="icon" title="Back" onclick={backToUsage}>←</button>
    {/if}
    <button class="icon" title="Hide to tray" onclick={() => invoke('hide_window')}>✕</button>
  </header>

  {#if view === 'popup'}
    <div class="cards" bind:this={cardsEl}>
      {#if snapshots.length === 0}
        <p class="empty">
          No providers enabled yet — open <button class="link" onclick={openSettings}>Settings</button> to add one.
        </p>
      {:else}
        {#each snapshots as snap (snap.provider_id)}
          <ProviderCard {snap} />
        {/each}
      {/if}
    </div>
  {:else}
    {#if appConfig}
      <Settings initialConfig={appConfig} {snapshots} onclose={leaveSettings} />
    {:else}
      <p class="empty">Loading…</p>
    {/if}
  {/if}
</main>
