<script>
  import { onMount, tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { LogicalSize } from '@tauri-apps/api/dpi';
  import ProviderCard from './lib/shared/UsageCard.svelte';
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

  // First-run desktop integration, AppImage only. Shown once: whichever way it
  // is answered, Rust records that the question was put, so "Not now" is
  // remembered as firmly as yes and no launch nags again. Settings keeps the
  // explicit add and remove controls, so a deferral is never final.
  let integrationPrompt = $state(null);
  let integrationResult = $state('');

  // Set when Rust found a config.json it could not read or parse. The file is
  // still on disk untouched and every ordinary save refuses, so this banner is
  // both the notice that the settings on screen are defaults and the only way
  // to get out of that state deliberately.
  let configRecovery = $state(null);
  let recoveryError = $state('');
  let recoveryResult = $state('');

  async function replaceBrokenConfig() {
    recoveryError = '';
    try {
      const kept = await invoke('recover_config', { config: $state.snapshot(appConfig) });
      configRecovery = null;
      recoveryResult = kept
        ? `Settings replaced. Your unreadable file was kept at ${kept}.`
        : 'Settings replaced.';
    } catch (error) {
      // The banner stays: nothing was replaced, so the state it describes is
      // still true.
      recoveryError = String(error.message ?? error);
    }
  }

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

  // The popup shrinks to exactly fit the usage meters. Settings asks for no
  // size at all, which is how it keeps the height the usage view had a moment
  // ago: the window is one window, and resizing it under a view swap made the
  // two screens read as two windows. Settings used to claim a fixed height of
  // its own, and that jump is what this drops. Nothing here re-imposes a height
  // for the duration of the visit, so a manual resize inside Settings also
  // stands, and Back or Save & close hands the popup branch below its content
  // fitting straight back.
  async function fitToContent() {
    try {
      if (view !== 'popup' || !headerEl || !cardsEl) return;
      const win = getCurrentWindow();
      const h = Math.min(680, Math.max(120, headerEl.offsetHeight + cardsEl.scrollHeight + 2));
      await win.setSize(new LogicalSize(window.innerWidth, h));
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
      configRecovery = initial.config_recovery ?? null;
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

  <!-- The saved settings could not be read. This says so wherever the user
       looks first, because the alternative — a widget that quietly comes up
       with nobody's accounts configured — reads as data loss that already
       happened. Nothing here deletes anything: replacing keeps the original. -->
  {#if configRecovery}
    <div class="config-recovery" role="alert">
      <p>
        <strong>Your saved settings could not be
        {configRecovery.kind === 'unreadable' ? 'read' : 'understood'}.</strong>
        They have been left untouched at <code>{configRecovery.path}</code> and the widget is
        running on defaults. Saving is blocked until you replace them, so nothing overwrites the
        original.
      </p>
      <p class="recovery-detail">{configRecovery.detail}</p>
      {#if recoveryError}<p class="recovery-detail">Could not replace: {recoveryError}</p>{/if}
      <div class="recovery-actions">
        <button onclick={replaceBrokenConfig}>Replace with these settings</button>
      </div>
    </div>
  {:else if recoveryResult}
    <p class="integration-result">{recoveryResult}</p>
  {/if}

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
          No providers enabled yet — open <button class="link" onclick={openSettings}>Settings</button> to add one.
        </p>
      {:else}
        {#each snapshots as snap (snap.provider_id)}
          <ProviderCard {snap} schedule={appConfig?.providers?.[snap.provider_id]?.usage_schedule} />
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
