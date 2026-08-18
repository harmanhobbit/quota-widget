// Headless mount smoke test for the Svelte frontend.
//
// `vite build` only type-free-compiles the components; it cannot tell you that
// a component throws while rendering. Svelte 5 has a class of runtime-only
// errors (state_unsafe_mutation, DataCloneError on state proxies) that compile
// clean and then abort the render, which shows up in the app as "the page
// doesn't open" rather than as any build failure. This mounts each top-level
// component against jsdom with the Tauri IPC stubbed, so those throw here.
//
// Requires jsdom, which is deliberately NOT a package.json dependency —
// adding one forces an npmDeps.hash regen in nix/package.nix. Install it
// on demand:
//
//   npm i -D jsdom --no-save
//   node --conditions=browser scripts/smoke-mount.mjs
//
// `--conditions=browser` is required: without it Node resolves Svelte's
// server build and every mount dies with `lifecycle_function_unavailable`.

import { JSDOM } from 'jsdom';
import { compile } from 'svelte/compiler';
import { readFileSync, readdirSync, writeFileSync, mkdirSync, rmSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';

const ROOT = new URL('..', import.meta.url).pathname;
const WORK = join(ROOT, '.smoke-mount');

// Config shaped like what get_snapshots returns. Keep in sync with
// crates/quota-core/src/config.rs if fields are added.
const CONFIG = {
  version: 2,
  poll_interval_secs: 60,
  autostart: false,
  hide_on_blur: false,
  mini_summary_bars: true,
  scroll_opacity: true,
  scroll_opacity_invert: false,
  sort_order: 'manual',
  sort_basis: 'icon',
  desktop_integration_prompted: false,
  thresholds: { warn_pct: 80, critical_pct: 95 },
  alerts: { toast: true, tray_color: true, auto_popup: false },
  providers: {
    // Two headlines: both must render, with the account name on the first row only.
    claude: provider({ enabled: true, mini_summary_metrics: ['window:five_hour', 'window:weekly'] }),
    // The selected weekly window is absent from the snapshot, so this account
    // has nothing left to show and says so rather than reading as 0%.
    codex: provider({ enabled: true, mini_summary_metrics: ['window:weekly'] }),
    openrouter: provider({}),
    hermes: provider({ mini_summary_metrics: ['window:monthly_allowance'] }),
  },
};

function provider(over) {
  return {
    kind: null,
    label: null,
    enabled: false,
    in_tray: true,
    thresholds: null,
    alerts: null,
    low_balance_warn: null,
    mini_summary_metric: null,
    mini_summary_metrics: null,
    tray_metric: null,
    settings: {},
    ...over,
  };
}

const SNAPSHOTS = [
  {
    provider_id: 'claude',
    provider_name: 'Claude',
    error: null,
    credits: null,
    windows: [
      // Bounded period: exercises the progress marker. Anchored to now so the
      // fraction is a stable mid-window value rather than drifting off the end.
      {
        metric_id: 'five_hour',
        label: '5h',
        used_pct: 42,
        informational: false,
        allowance: { remaining: 1025, total: 1000, unit: 'credits' },
        period_start: new Date(Date.now() - 60 * 60_000).toISOString(),
        resets_at: new Date(Date.now() + 4 * 60 * 60_000).toISOString(),
      },
      // A reset time but no period start — the marker must be skipped, not
      // rendered at a nonsense position.
      {
        metric_id: 'weekly',
        label: 'Weekly',
        used_pct: 88,
        informational: false,
        resets_at: new Date(Date.now() + 3 * 24 * 60 * 60_000).toISOString(),
      },
    ],
  },
  {
    provider_id: 'codex',
    provider_name: 'Codex',
    error: null,
    credits: null,
    windows: [{ metric_id: 'five_hour', label: '5h', used_pct: 70, informational: false }],
  },
  {
    provider_id: 'openrouter',
    provider_name: 'OpenRouter',
    error: null,
    credits: { balance: 3.42, unit: 'USD' },
    windows: [],
  },
  {
    provider_id: 'hermes',
    provider_name: 'Hermes Portal',
    error: null,
    credits: { balance: 12, unit: 'USD' },
    windows: [{ metric_id: 'monthly_allowance', label: 'Monthly allowance (Plus)', used_pct: 100, informational: true }],
  },
  // A spend-reporting provider with no budget set: labelled credits, which must
  // render as "Cost this month: …" rather than as a remaining balance.
  {
    provider_id: 'fireworks',
    provider_name: 'Fireworks',
    error: null,
    credits: { balance: 8.75, label: 'Cost this month', unit: 'USD' },
    windows: [],
  },
];

// Installing crosses a dynamic import and several awaits, so a single
// macrotask does not reliably get from the click to the rendered result. Turn
// the loop a few times rather than guessing at one long sleep.
async function settle(flushSync, turns = 5) {
  for (let i = 0; i < turns; i++) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    flushSync();
  }
}

// Every component under test, with the props App would really pass. Settings
// takes its config as a prop that has been through $state in App, so it
// arrives as a proxy — mirror that exactly, since it is what broke it before.
const CASES = [
  {
    file: 'src/App.svelte',
    props: () => ({}),
    buildBranch: 'smoke-branch',
    expect: ['smoke-branch'],
    verify: async ({ target, flushSync, emit }) => {
      const main = target.querySelector('main');
      const opacity = () => document.documentElement.style.getPropertyValue('--window-opacity');
      main.dispatchEvent(new window.WheelEvent('wheel', { deltaY: -100, bubbles: true, cancelable: true }));
      flushSync();
      if (opacity() !== '0.92') throw new Error(`scroll did not fade popup: ${opacity()}`);
      // The full popup's show-time reset is unchanged: unlike the summary, it
      // opens opaque every time it is shown.
      await emit('window-shown', 'popup');
      flushSync();
      if (opacity() !== '1') throw new Error(`popup kept its fade across a show: ${opacity()}`);
    },
  },
  {
    file: 'src/App.svelte',
    props: () => ({}),
    config: { ...CONFIG, scroll_opacity: false },
    verify: ({ target, flushSync }) => {
      const main = target.querySelector('main');
      const before = document.documentElement.style.getPropertyValue('--window-opacity');
      main.dispatchEvent(new window.WheelEvent('wheel', { deltaY: -100, bubbles: true, cancelable: true }));
      flushSync();
      const after = document.documentElement.style.getPropertyValue('--window-opacity');
      if (after !== before) throw new Error('disabled opacity setting still changed the popup');
    },
  },
  // Inverted, the same upward flick that faded above must restore instead —
  // and since the popup opens fully opaque, that means no change at all.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    config: { ...CONFIG, scroll_opacity_invert: true },
    verify: ({ target, flushSync }) => {
      const main = target.querySelector('main');
      const opacity = () => document.documentElement.style.getPropertyValue('--window-opacity');
      const wheel = (deltaY) =>
        main.dispatchEvent(new window.WheelEvent('wheel', { deltaY, bubbles: true, cancelable: true }));
      wheel(-100);
      flushSync();
      if (opacity() !== '1') throw new Error(`inverted scroll up faded the popup: ${opacity()}`);
      wheel(100);
      flushSync();
      if (opacity() !== '0.92') throw new Error(`inverted scroll down did not fade: ${opacity()}`);
    },
  },
  // An unreadable config.json: the settings on screen are defaults, not the
  // user's, and the popup has to say so — a silent fallback is what reads as
  // data already lost.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    configRecovery: {
      kind: 'malformed',
      path: '/home/u/.config/quota-widget/config.json',
      detail: 'expected value at line 1 column 2',
    },
    recoveryKept: '/home/u/.config/quota-widget/config.json.unreadable',
    verify: async ({ target, flushSync }) => {
      const banner = target.querySelector('.config-recovery');
      if (!banner) throw new Error('an unreadable config was not reported to the user');
      // It must name the file being kept and say the app is on defaults.
      if (!banner.textContent.includes('/home/u/.config/quota-widget/config.json')) {
        throw new Error(`the banner did not name the kept file: ${banner.textContent}`);
      }
      if (!/defaults/.test(banner.textContent)) {
        throw new Error(`the banner did not say the settings are defaults: ${banner.textContent}`);
      }
      if (globalThis.__SMOKE_SHELL_CALLS__.includes('recover_config')) {
        throw new Error('the config was replaced before the user asked for it');
      }
      banner.querySelector('button').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      flushSync();
      if (!globalThis.__SMOKE_SHELL_CALLS__.includes('recover_config')) {
        throw new Error('the recovery button did not reach recover_config');
      }
      if (target.querySelector('.config-recovery')) {
        throw new Error('the banner stayed up after a successful recovery');
      }
      // Where the original went is the only way back to those accounts.
      if (!target.textContent.includes('config.json.unreadable')) {
        throw new Error(`recovery did not say where the original was kept: ${target.textContent}`);
      }
    },
  },
  // A failed recovery changed nothing, so the banner it describes must stay.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    configRecovery: { kind: 'unreadable', path: '/home/u/.config/quota-widget/config.json', detail: 'Permission denied (os error 13)' },
    recoverError: 'Permission denied (os error 13)',
    verify: async ({ target, flushSync }) => {
      target.querySelector('.config-recovery button').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      flushSync();
      const banner = target.querySelector('.config-recovery');
      if (!banner) throw new Error('a failed recovery dismissed the warning anyway');
      if (!banner.textContent.includes('Permission denied')) {
        throw new Error(`the failure was not shown: ${banner.textContent}`);
      }
    },
  },
  // The ordinary case: nothing wrong on disk, no banner, nothing to recover.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    verify: ({ target }) => {
      if (target.querySelector('.config-recovery')) {
        throw new Error('a readable config still showed a recovery warning');
      }
    },
  },
  // First-run desktop integration (AppImage only). A non-AppImage build gets
  // null from the status command and must never see the question.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    verify: ({ target }) => {
      if (target.querySelector('.integration-prompt')) {
        throw new Error('a build with no launcher to manage still asked about one');
      }
    },
  },
  // An AppImage with no launcher yet asks before writing anything, and taking
  // the offer reaches the add command — consent first, files after.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    desktop: { state: 'absent', appimage: '/home/u/Downloads/q.AppImage', desktop_file: '/home/u/.local/share/applications/quota-widget.desktop' },
    // Asserted here rather than through `expect`, which runs after verify has
    // deliberately dismissed the prompt.
    verify: async ({ target, flushSync }) => {
      const button = (text) => [...target.querySelectorAll('.integration-prompt button')]
        .find((b) => b.textContent.trim() === text);
      const prompt = target.querySelector('.integration-prompt');
      if (!/applications menu/.test(prompt?.textContent ?? '')) {
        throw new Error(`the prompt did not say what it would do: ${prompt?.textContent}`);
      }
      if (!button('Not now') || !button('Add it')) {
        throw new Error('the prompt did not offer both answers');
      }
      if (globalThis.__SMOKE_DESKTOP_CALLS__.length) {
        throw new Error('the prompt wrote something before it was answered');
      }
      button('Add it').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      flushSync();
      if (!globalThis.__SMOKE_DESKTOP_CALLS__.includes('add')) {
        throw new Error('accepting did not reach desktop_integration_add');
      }
      // The deferral is recorded either way, or the next launch asks again.
      if (!globalThis.__SMOKE_DESKTOP_CALLS__.includes('prompted')) {
        throw new Error('answering did not record that the question was put');
      }
      if (target.querySelector('.integration-prompt')) throw new Error('the prompt stayed up after being answered');
    },
  },
  // Declining writes nothing but is still remembered, so it cannot nag.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    desktop: { state: 'absent', appimage: '/home/u/Downloads/q.AppImage', desktop_file: '/home/u/.local/share/applications/quota-widget.desktop' },
    verify: async ({ target, flushSync }) => {
      [...target.querySelectorAll('.integration-prompt button')]
        .find((b) => b.textContent.trim() === 'Not now').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      flushSync();
      if (globalThis.__SMOKE_DESKTOP_CALLS__.includes('add')) {
        throw new Error('declining still added the launcher');
      }
      if (!globalThis.__SMOKE_DESKTOP_CALLS__.includes('prompted')) {
        throw new Error('a deferral was not remembered');
      }
      if (target.querySelector('.integration-prompt')) throw new Error('declining left the prompt up');
    },
  },
  // Already asked once: never again, whatever the launcher state is now.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    config: { ...CONFIG, desktop_integration_prompted: true },
    desktop: { state: 'absent', appimage: '/home/u/Downloads/q.AppImage', desktop_file: '/home/u/.local/share/applications/quota-widget.desktop' },
    verify: ({ target }) => {
      if (target.querySelector('.integration-prompt')) {
        throw new Error('a remembered deferral did not suppress the prompt');
      }
    },
  },
  // A moved AppImage is a Settings decision, not an opening question: repair
  // changes an existing user file, so it is never proposed unprompted.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    desktop: { state: 'stale', target: '/home/u/old/q.AppImage', appimage: '/home/u/Downloads/q.AppImage', desktop_file: '/home/u/.local/share/applications/quota-widget.desktop' },
    verify: ({ target }) => {
      if (target.querySelector('.integration-prompt')) {
        throw new Error('a moved AppImage was offered a first-run prompt rather than a Settings repair');
      }
    },
  },
  // Claude shows both selected headlines as separate rows; Codex's only
  // selected window is missing from its snapshot, so it reads "no data";
  // OpenRouter is Automatic and falls through to its credit balance.
  {
    file: 'src/lib/MiniSummary.svelte',
    props: () => ({}),
    // Fireworks' labelled spend takes "Cost this month" as its row label,
    // where an unlabelled balance shows the bare currency.
    expect: ['42%', '5h', '88%', 'Weekly', 'no data', '$3.42', 'USD', '100%', 'Monthly allowance (Plus)', '$8.75', 'Cost this month'],
    verify: ({ target, flushSync }) => {
      const names = [...target.querySelectorAll('.hover-name')].map((el) => el.textContent);
      // Claude's second row must leave the name blank rather than repeat it.
      if (names.slice(0, 3).join('|') !== 'Claude||Codex') {
        throw new Error(`name column was ${names.join('|')}`);
      }
      const cells = [...target.querySelectorAll('.hover-bar-cell')];
      // Claude's 5-hour window is one hour into five, so its marker sits at
      // 20%. The cell lays the bar out to fill its width, so a percentage of
      // the cell puts the marker exactly where a percentage of the bar did.
      const mark = cells[0].querySelector('.period-mark');
      if (!mark) throw new Error('bounded window rendered no period marker');
      const pct = parseFloat(mark.style.left);
      if (!(Math.abs(pct - 20) < 1)) throw new Error(`marker sat at ${mark.style.left}, expected ~20%`);
      // The weekly window has no period_start, so it gets no marker at all
      // rather than one pinned at an end.
      if (cells[1].querySelector('.period-mark')) {
        throw new Error('window without a period start still drew a marker');
      }
      // The hover target exists only where there is a marker to describe, and
      // must be a sibling of the bar rather than a child: the bar clips its
      // overflow, and hit-testing follows that clip.
      if (!cells[0].querySelector(':scope > .hover-bar-target')) {
        throw new Error('bounded window rendered no hover target');
      }
      if (cells[1].querySelector('.hover-bar-target')) {
        throw new Error('window without a period start still drew a hover target');
      }
      // The marker hangs off the cell, not the bar: the bar is 5px tall and
      // clips its overflow, so a marker inside it could never grow taller.
      // Its size and centring are CSS, which jsdom cannot report — only the
      // parentage that makes them possible is assertable here.
      if (!cells[0].querySelector(':scope > .period-mark')) {
        throw new Error('marker is not parented where it can grow past the bar');
      }
      // jsdom reports a zero-sized bar, so the pointer reads as sitting on the
      // marker — enough to prove the tooltip arms and states both of its parts.
      const targetEl = cells[0].querySelector('.hover-bar-target');
      targetEl.dispatchEvent(new window.PointerEvent('pointermove', { bubbles: true, clientX: 0 }));
      flushSync();
      const tip = targetEl.getAttribute('data-tip') ?? '';
      if (!/through/.test(tip) || !/resets/.test(tip)) {
        throw new Error(`tooltip read ${JSON.stringify(tip)}`);
      }
      // `data-armed` is what shows it; the text is held constant so the
      // pseudo-element's box survives the gesture and has something to fade.
      if (targetEl.getAttribute('data-armed') == null) {
        throw new Error('pointer on the marker did not arm the tooltip');
      }
      // The OS tooltip is gone rather than merely covered; both would show,
      // the native one arriving late on top of the instant one.
      if (targetEl.getAttribute('title')) {
        throw new Error('native tooltip survived alongside the styled one');
      }
      targetEl.dispatchEvent(new window.PointerEvent('pointerleave', { bubbles: true }));
      flushSync();
      if (targetEl.getAttribute('data-armed') != null) {
        throw new Error('tooltip stayed armed after the pointer left the bar');
      }
      // The text outlives the gesture by design — only the arming toggles.
      if (!targetEl.getAttribute('data-tip')) {
        throw new Error('tooltip text should persist so its box can fade');
      }
    },
  },
  // The Settings return state, exercised through the app's navigation seam:
  // what is on screen after the exit, and what the exit asked the shell to do.
  // Direct entry from the usage popup returns to the usage popup and involves
  // the shell not at all — the window it would act on is already the right one.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    verify: async ({ target, flushSync, shellCalls }) => {
      const settingsButton = () => target.querySelector('button[title="Settings"]');
      const inSettings = () => Boolean(target.querySelector('.settings'));
      const warnInput = () => [...target.querySelectorAll('.settings input.num')]
        .find((el) => el.closest('label')?.textContent.includes('Warn at'));
      for (const exit of ['back', 'escape', 'save']) {
        settingsButton().click();
        flushSync();
        if (!inSettings()) throw new Error(`did not reach Settings before the ${exit} exit`);
        // An unsaved edit, so Back and Escape can be shown to discard it: the
        // next visit must read the persisted value, not this draft.
        const edit = warnInput();
        edit.value = '31';
        edit.dispatchEvent(new window.Event('input', { bubbles: true }));
        flushSync();
        if (exit === 'back') target.querySelector('button[title="Back"]').click();
        else if (exit === 'escape') {
          window.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
        } else {
          target.querySelector('.settings .primary').click();
          await new Promise((resolve) => setTimeout(resolve, 0));
        }
        flushSync();
        if (inSettings()) throw new Error(`${exit} did not leave Settings`);
        if (!target.querySelector('.cards')) throw new Error(`${exit} did not return to the usage popup`);
        if (shellCalls().length) {
          throw new Error(`${exit} from the popup drove the shell: ${shellCalls().join(',')}`);
        }
        // Only Save commits. Back and Escape reach Rust with nothing, and the
        // next visit opens on the unedited config — the discard being visible
        // rather than merely unpersisted.
        const saved = globalThis.__SMOKE_LAST_CONFIG__?.thresholds.warn_pct;
        if (exit === 'save' ? saved !== 31 : saved === 31) {
          throw new Error(`${exit} persisted warn_pct ${saved}`);
        }
        globalThis.__SMOKE_LAST_CONFIG__ = undefined;
        settingsButton().click();
        flushSync();
        if (exit !== 'save' && warnInput().value !== String(CONFIG.thresholds.warn_pct)) {
          throw new Error(`${exit} left its unsaved edit in the reopened form`);
        }
        target.querySelector('button[title="Back"]').click();
        flushSync();
        shellCalls().length = 0;
      }
    },
  },
  // Tray entry carries the return state with the navigation. A mini summary —
  // pinned or transient, a distinction the frontend never sees — comes back
  // through the shell when the visit *ends*, and the full window is not left
  // showing the usage list behind it.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    verify: async ({ target, flushSync, emit, shellCalls }) => {
      const inSettings = () => Boolean(target.querySelector('.settings'));
      for (const [returnTo, exit] of [['mini', 'escape'], ['mini', 'save'], ['hidden', 'escape'], ['hidden', 'save']]) {
        shellCalls().length = 0;
        emit('navigate', { view: 'settings', return_to: returnTo });
        if (!inSettings()) throw new Error(`tray navigation did not open Settings (${returnTo})`);
        if (exit === 'escape') {
          window.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
        } else {
          target.querySelector('.settings .primary').click();
          await new Promise((resolve) => setTimeout(resolve, 0));
        }
        flushSync();
        if (inSettings()) throw new Error(`${returnTo}/${exit} did not leave Settings`);
        if (shellCalls().join(',') !== `exit_settings:${returnTo}`) {
          throw new Error(`${returnTo}/${exit} drove the shell as ${shellCalls().join(',') || 'nothing'}`);
        }
      }
    },
  },
  // Back is navigation, not an exit. Even from a tray visit whose return state
  // is the mini summary, it lands on the usage list in the window that is
  // already open and asks the shell for nothing — the user came to look at the
  // widget, and Back is how they get to it.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    verify: ({ target, flushSync, emit, shellCalls }) => {
      for (const returnTo of ['mini', 'hidden']) {
        shellCalls().length = 0;
        emit('navigate', { view: 'settings', return_to: returnTo });
        target.querySelector('button[title="Back"]').click();
        flushSync();
        if (target.querySelector('.settings')) throw new Error(`Back (${returnTo}) stayed in Settings`);
        if (!target.querySelector('.cards')) throw new Error(`Back (${returnTo}) did not land on the usage list`);
        if (shellCalls().length) {
          throw new Error(`Back (${returnTo}) drove the shell as ${shellCalls().join(',')}`);
        }
        // The visit ended with the window still up, so its capture ended too.
        // Escape on the usage list is an ordinary hide — never a restore of the
        // summary that visit came from.
        window.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
        flushSync();
        if (shellCalls().join(',') !== 'hide_window') {
          throw new Error(`a capture outlived Back (${returnTo}): ${shellCalls().join(',') || 'nothing'}`);
        }
        // Re-entering through the gear is a fresh, direct visit: leaving it
        // returns to the usage list, not to the summary the *tray* visit was
        // carrying before Back ended it.
        shellCalls().length = 0;
        target.querySelector('button[title="Settings"]').click();
        flushSync();
        window.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
        flushSync();
        if (shellCalls().length) {
          throw new Error(`Back (${returnTo}) left its capture for the next visit: ${shellCalls().join(',')}`);
        }
      }
    },
  },
  // The window is one window: entering Settings must not resize it, so the
  // height the usage list was showing is the height Settings is read at. Both
  // routes in are checked — the gear and the tray's `navigate` — because they
  // set the view up differently and only the sizing effect is shared. Coming
  // back re-fits to the usage content, which is the popup asking for a size of
  // its own again rather than inheriting whatever Settings was left at.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    verify: async ({ target, flushSync, emit, sizes }) => {
      const enter = {
        gear: () => target.querySelector('button[title="Settings"]').click(),
        tray: () => emit('navigate', { view: 'settings', return_to: 'popup' }),
      };
      for (const [route, open] of Object.entries(enter)) {
        await settle(flushSync);
        const fitted = sizes().at(-1);
        if (!fitted) throw new Error(`the usage list never sized the window (${route})`);
        sizes().length = 0;
        open();
        flushSync();
        await settle(flushSync);
        if (!target.querySelector('.settings')) throw new Error(`${route} did not reach Settings`);
        if (sizes().length) {
          throw new Error(`${route} into Settings resized the window to ${JSON.stringify(sizes())}`);
        }
        target.querySelector('button[title="Back"]').click();
        flushSync();
        await settle(flushSync);
        const back = sizes().at(-1);
        if (!back) throw new Error(`returning from Settings (${route}) never re-fitted the usage list`);
        if (back.height !== fitted.height) {
          throw new Error(`the usage list came back at ${back.height}, not its content height ${fitted.height}`);
        }
      }
    },
  },
  // ✕ stays an explicit hide-to-tray whatever the captured return state is: it
  // means "no window", not "put back what was there".
  {
    file: 'src/App.svelte',
    props: () => ({}),
    verify: ({ target, emit, shellCalls }) => {
      emit('navigate', { view: 'settings', return_to: 'mini' });
      target.querySelector('button[title="Hide to tray"]').click();
      if (shellCalls().join(',') !== 'hide_window') {
        throw new Error(`✕ from Settings ran ${shellCalls().join(',') || 'nothing'}`);
      }
    },
  },
  // A failed save keeps Settings mounted with its error, and keeps the captured
  // return state: retrying successfully still lands on the mini summary.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    saveError: 'disk on fire',
    verify: async ({ target, flushSync, emit, shellCalls }) => {
      emit('navigate', { view: 'settings', return_to: 'mini' });
      target.querySelector('.settings .primary').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      flushSync();
      if (!target.querySelector('.settings')) throw new Error('a failed save closed Settings');
      if (!target.querySelector('.settings .test.bad')) throw new Error('a failed save showed no error');
      if (shellCalls().length) throw new Error(`a failed save drove the shell: ${shellCalls().join(',')}`);
      globalThis.__SMOKE_SAVE_ERROR__ = '';
      target.querySelector('.settings .primary').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      flushSync();
      if (shellCalls().join(',') !== 'exit_settings:mini') {
        throw new Error(`the retry lost the return state: ${shellCalls().join(',') || 'nothing'}`);
      }
    },
  },
  // A reopen after a tray Settings visit starts a fresh visit: `window-shown`
  // clears the previous capture, so the next exit does not restore a summary
  // belonging to a visit that already ended.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    verify: ({ target, flushSync, emit, shellCalls }) => {
      emit('navigate', { view: 'settings', return_to: 'mini' });
      emit('window-shown', undefined);
      if (target.querySelector('.settings')) throw new Error('a reshow left the window in Settings');
      target.querySelector('button[title="Settings"]').click();
      flushSync();
      shellCalls().length = 0;
      // Escape, not Back: Back asks the shell for nothing either way, so it
      // could not tell a cleared capture from a surviving one.
      window.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
      flushSync();
      if (shellCalls().length) {
        throw new Error(`a stale return state survived the reshow: ${shellCalls().join(',')}`);
      }
    },
  },
  {
    file: 'src/lib/shared/UsageCard.svelte',
    props: () => ({
      snap: {
        provider_name: 'Firecrawl',
        fetched_at: new Date().toISOString(),
        error: null,
        credits: null,
        windows: [{
          metric_id: 'monthly_credits', label: 'Credits', used_pct: -3,
          resets_at: null, period_start: null, informational: false,
          allowance: { remaining: 1025, total: 1000, unit: 'credits' },
        }],
      },
    }),
    expect: ['-3%', '1,025 / 1,000 credits remaining'],
    verify: ({ target }) => {
      const fill = target.querySelector('.fill');
      if (fill?.style.width !== '0%') {
        throw new Error(`negative usage drew ${fill?.style.width ?? 'no'} bar instead of 0%`);
      }
    },
  },
  {
    file: 'src/lib/MiniSummary.svelte',
    props: () => ({}),
    verify: async ({ target, flushSync, emit }) => {
      const mini = target.querySelector('.mini');
      const opacity = () => document.documentElement.style.getPropertyValue('--window-opacity');
      const wheel = () =>
        mini.dispatchEvent(new window.WheelEvent('wheel', { deltaY: -100, bubbles: true, cancelable: true }));
      // A newly mounted summary — the fresh-app-start case — opens opaque.
      if (opacity() !== '1') throw new Error(`summary did not open opaque: ${opacity()}`);
      wheel();
      flushSync();
      if (opacity() !== '0.92') throw new Error(`scroll did not fade summary: ${opacity()}`);
      // Every native hide/show path ends in this one notification, so playing
      // it back is what proves the level survives tray toggles, click-away,
      // the close button, and the temporary hide while the popup is open.
      await emit('mini-shown', null);
      flushSync();
      if (opacity() !== '0.92') throw new Error(`showing the summary reset its fade to ${opacity()}`);
      // Even unpinned it stops at the shared floor rather than nothing: the
      // level outlives every hide now, so a zero floor would mean reopening
      // an invisible window indefinitely.
      for (let i = 0; i < 20; i += 1) wheel();
      flushSync();
      if (Number(opacity()) !== 0.15) throw new Error(`unpinned summary floored at ${opacity()}`);
      // The floor is retained too — the case most likely to be "helpfully"
      // reset, and the one that has to stay findable after a reopen.
      await emit('mini-shown', null);
      flushSync();
      if (Number(opacity()) !== 0.15) throw new Error(`a fully faded summary came back at ${opacity()}`);
      // Turning the preference off is still the escape hatch, and turning it
      // back on must not restore the level that was just cleared.
      await emit('config', { ...CONFIG, scroll_opacity: false });
      flushSync();
      if (opacity() !== '1') throw new Error(`disabling fade left the summary at ${opacity()}`);
      await emit('config', CONFIG);
      flushSync();
      if (opacity() !== '1') throw new Error(`re-enabling fade restored a stale level: ${opacity()}`);
    },
  },
  { file: 'src/lib/MiniSummary.svelte', props: () => ({}), snapshotsError: true, expect: ['Could not load summary'] },
  {
    file: 'src/lib/MiniSummary.svelte',
    props: () => ({}),
    config: { ...CONFIG, scroll_opacity: false },
    verify: ({ target, flushSync }) => {
      const mini = target.querySelector('.mini');
      const before = document.documentElement.style.getPropertyValue('--window-opacity');
      mini.dispatchEvent(new window.WheelEvent('wheel', { deltaY: -100, bubbles: true, cancelable: true }));
      flushSync();
      const after = document.documentElement.style.getPropertyValue('--window-opacity');
      if (after !== before) throw new Error('disabled opacity setting still changed the summary');
    },
  },
  { file: 'src/lib/MiniSummary.svelte', props: () => ({}), buildBranch: 'smoke-branch', expect: ['smoke-branch'] },
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    expect: ['Providers', 'Mini-summary headlines', 'Tray icon status', 'Worst of selected', 'Thresholds', 'Alerts', 'Fade windows when scrolling over them', 'Save'],
    verify: async ({ target, flushSync }) => {
      const findButton = (text) => [...target.querySelectorAll('button')].find((button) => button.textContent.trim() === text);
      const card = (name) => [...target.querySelectorAll('.provider')]
        .find((el) => el.querySelector('strong').textContent.trim() === name);
      // Accounts start collapsed, so every control below has to be revealed
      // first — and that default is itself worth asserting.
      if (target.querySelector('.metric-picker')) throw new Error('accounts did not start collapsed');
      card('Claude').querySelector('.provider-disclosure').click();
      flushSync();
      // The headline menu is built by hand rather than being a native control,
      // so open it and toggle an item to prove the wiring.
      const picker = target.querySelector('.metric-picker');
      if (picker.querySelector('.metric-menu')) throw new Error('headline menu started open');
      picker.querySelector('.metric-toggle').click();
      flushSync();
      const items = [...picker.querySelectorAll('.metric-item input')];
      if (items.length < 3) throw new Error(`headline menu had ${items.length} items`);
      // [0] is Automatic; Claude's two headlines follow, both checked.
      if (items[0].checked || !items[1].checked || !items[2].checked) {
        throw new Error('headline menu did not reflect the configured selection');
      }
      items[1].click();
      flushSync();
      if (picker.querySelector('.metric-toggle').textContent.trim() !== 'Weekly ▾') {
        throw new Error(`unchecking left the summary as ${picker.querySelector('.metric-toggle').textContent.trim()}`);
      }
      items[0].click();
      flushSync();
      if (picker.querySelector('.metric-toggle').textContent.trim() !== 'Automatic ▾') {
        throw new Error('checking Automatic did not clear the metric selection');
      }
      items[0].click();
      flushSync();
      if (picker.querySelector('.metric-toggle').textContent.trim() !== 'None ▾') {
        throw new Error('unchecking Automatic did not select no headlines');
      }
      picker.querySelector('.metric-toggle').click();
      flushSync();
      findButton('+ Add account').click();
      flushSync();
      const addName = target.querySelector('.add-account input');
      addName.value = 'Work Claude';
      addName.dispatchEvent(new window.Event('input', { bubbles: true }));
      findButton('Add account').click();
      flushSync();
      if (target.querySelector('.add-account') || !findButton('+ Add account')) {
        throw new Error('add-account panel did not collapse after adding an account');
      }
      target.querySelector('button[aria-label="Move Codex up"]').click();
      flushSync();
      card('OpenRouter').querySelector('.provider-disclosure').click();
      flushSync();
      [...card('OpenRouter').querySelectorAll('.provider-footer button')]
        .find((button) => button.textContent.trim() === 'Remove account').click();
      await Promise.resolve(); // let Remove's secret-clear await finish
      flushSync();
      target.querySelector('.primary').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      const saved = globalThis.__SMOKE_LAST_CONFIG__;
      if (!saved || Object.keys(saved.providers).join(',') !== 'codex,claude,hermes,claude#2') {
        throw new Error(`saved account order was ${saved ? Object.keys(saved.providers).join(',') : 'missing'}`);
      }
      if (saved.providers['claude#2'].label !== 'Work Claude') {
        throw new Error('new account label was not saved');
      }
      if (saved.providers.claude.mini_summary_metrics?.length !== 0) {
        throw new Error('unchecking Automatic did not save no headlines');
      }
    },
  },
  // The footer is fixed: only the form scrolls, and the commit action, the
  // save error and the version live outside the scrolling region so they are
  // on screen wherever the user has scrolled to. jsdom lays nothing out, so
  // what is assertable is the structure that makes it true — which region the
  // footer's parts belong to — rather than any measured position.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    expect: ['Quota Widget v0.0.0-test'],
    verify: ({ target }) => {
      const form = target.querySelector('.settings > .settings-form');
      const footer = target.querySelector('.settings > .settings-footer');
      if (!form) throw new Error('settings rendered no scrollable form region');
      if (!footer) throw new Error('settings rendered no fixed footer');
      if (!form.querySelector('section')) throw new Error('the fields are not inside the scrolling region');
      // Anything in the footer must not also be in the scrolling region.
      if (form.contains(footer)) throw new Error('the footer scrolls with the form');
      const save = [...footer.querySelectorAll('button')]
        .find((b) => b.textContent.trim() === 'Save & close');
      if (!save) throw new Error('Save & close is not in the fixed footer');
      if (!save.closest('.actions')) throw new Error('Save & close is not in the right-aligned action row');
      const version = footer.querySelector('.version');
      if (!version) throw new Error('the version is not in the fixed footer');
      if (!/Quota Widget v/.test(version.textContent)) {
        throw new Error(`footer version read ${JSON.stringify(version.textContent)}`);
      }
      // Centred beneath the action, so it follows the action row in order.
      if (!(save.closest('.actions').compareDocumentPosition(version) & 4 /* FOLLOWING */)) {
        throw new Error('the version is not beneath the action');
      }
    },
  },
  // A failed save keeps Settings mounted with the error visible in the footer,
  // rather than closing on a write that never landed.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({
      initialConfig: $.proxy(structuredClone(CONFIG)),
      snapshots: structuredClone(SNAPSHOTS),
      onclose() { globalThis.__SMOKE_CLOSED__ = true; },
    }),
    saveError: 'disk is full',
    verify: async ({ target, flushSync }) => {
      globalThis.__SMOKE_CLOSED__ = false;
      globalThis.__SMOKE_LAST_CONFIG__ = undefined;
      target.querySelector('.primary').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      flushSync();
      if (globalThis.__SMOKE_CLOSED__) throw new Error('a failed save still closed Settings');
      const footer = target.querySelector('.settings > .settings-footer');
      if (!footer?.textContent.includes('disk is full')) {
        throw new Error(`the save error is not visible in the footer: ${footer?.textContent}`);
      }
      // Still the whole panel, not a stub left over from a mid-render throw.
      if (!target.querySelector('.settings-form section')) {
        throw new Error('the form did not survive the failed save');
      }
      if (!footer.querySelector('.version')) throw new Error('the version left the footer on a failed save');
    },
  },
  // Ordering: the basis select is inert while the order is Manual, both
  // selects reach Rust as the snake_case strings serde expects, and a config
  // predating the feature fills them in rather than saving nulls.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => {
      const old = structuredClone(CONFIG);
      delete old.sort_order;
      delete old.sort_basis;
      return { initialConfig: $.proxy(old), snapshots: structuredClone(SNAPSHOTS), onclose() {} };
    },
    expect: ['Order accounts by', 'Sorting on', 'Manual (my order)', 'Expiry: soonest first'],
    verify: async ({ target, flushSync }) => {
      const selects = [...target.querySelectorAll('select')];
      const orderSelect = selects.find((el) => el.value === 'manual');
      const basisSelect = selects.find((el) => el.value === 'icon');
      if (!orderSelect || !basisSelect) {
        throw new Error('a config without the sort fields left the selects unset');
      }
      if (!basisSelect.disabled) throw new Error('basis select was live under Manual');
      orderSelect.value = 'expiry_soonest';
      orderSelect.dispatchEvent(new window.Event('change', { bubbles: true }));
      flushSync();
      if (basisSelect.disabled) throw new Error('basis select stayed disabled under a real order');
      basisSelect.value = 'worst_case';
      basisSelect.dispatchEvent(new window.Event('change', { bubbles: true }));
      flushSync();
      target.querySelector('.primary').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      const saved = globalThis.__SMOKE_LAST_CONFIG__;
      if (saved?.sort_order !== 'expiry_soonest' || saved?.sort_basis !== 'worst_case') {
        throw new Error(`saved sort was ${saved?.sort_order}/${saved?.sort_basis}`);
      }
    },
  },
  // The invert toggle is inert while fading itself is off, and a config
  // predating the field saves a real boolean rather than undefined.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => {
      const old = structuredClone(CONFIG);
      delete old.scroll_opacity_invert;
      return { initialConfig: $.proxy(old), snapshots: structuredClone(SNAPSHOTS), onclose() {} };
    },
    expect: ['Reverse which scroll direction fades'],
    verify: async ({ target, flushSync }) => {
      const boxes = [...target.querySelectorAll('input[type="checkbox"]')];
      const label = (box) => box.closest('label')?.textContent.trim();
      const fade = boxes.find((b) => label(b) === 'Fade windows when scrolling over them');
      const invert = boxes.find((b) => label(b) === 'Reverse which scroll direction fades');
      if (!fade || !invert) throw new Error('scroll fade toggles were not both rendered');
      if (invert.disabled) throw new Error('invert was disabled while fading was on');
      if (invert.checked) throw new Error('a config without the field defaulted invert to on');
      fade.click();
      flushSync();
      if (!invert.disabled) throw new Error('invert stayed live with fading off');
      fade.click();
      flushSync(); // re-enable before clicking, or the disabled input eats it
      invert.click();
      flushSync();
      target.querySelector('.primary').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      const saved = globalThis.__SMOKE_LAST_CONFIG__;
      if (saved?.scroll_opacity_invert !== true) {
        throw new Error(`saved invert was ${saved?.scroll_opacity_invert}`);
      }
    },
  },
  // An installed Windows build offers Install; a build that cannot replace
  // itself must offer the upgrade instructions instead, and must never render
  // an install button it cannot honour.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    update: {
      available: true, current: '0.1.0', latest: '0.2.0', notes: 'n',
      pub_date: '2026-08-05T09:21:10Z', url: 'https://example.test/setup.exe',
      installable: true, restart_after_install: false,
    },
    expect: ['Update available: v0.2.0', 'Install update'],
    verify: async ({ target, flushSync }) => {
      const install = [...target.querySelectorAll('button')]
        .find((b) => b.textContent.trim() === 'Install update');
      if (!install) throw new Error('an installable update offered no Install button');
      install.click();
      await settle(flushSync);
      if (!globalThis.__SMOKE_INSTALLED__) throw new Error('Install update did not reach downloadAndInstall');
      // The NSIS installer relaunches the app itself, so no restart is offered
      // and the message says the app comes back on its own.
      const restart = [...target.querySelectorAll('button')]
        .find((b) => b.textContent.trim() === 'Restart now');
      if (restart) throw new Error('Windows offered a manual restart the installer performs itself');
      if (!globalThis.__SMOKE_INSTALLING_TEXT__.includes('close and reopen')) {
        throw new Error('Windows install did not warn the app is about to exit and relaunch');
      }
    },
  },
  // An AppImage is the Linux installable artifact. It installs in place, so
  // the old version keeps running: completion must offer Restart now and
  // Later, and must never claim the app relaunched itself.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    update: {
      available: true, current: '0.1.0', latest: '0.2.0', notes: 'n',
      pub_date: '2026-08-05T09:21:10Z', url: 'https://example.test/app.AppImage',
      installable: true, restart_after_install: true,
    },
    expect: ['Update available: v0.2.0', 'Install update'],
    verify: async ({ target, flushSync }) => {
      const button = (label) => [...target.querySelectorAll('button')]
        .find((b) => b.textContent.trim() === label);
      if (button('Restart now')) throw new Error('offered a restart before anything was installed');
      button('Install update').click();
      await settle(flushSync);
      if (!globalThis.__SMOKE_INSTALLED__) throw new Error('Install update did not reach downloadAndInstall');
      // Checked over the whole run, not just the end state: telling a Linux
      // user the app will reopen by itself is wrong even transiently.
      if (globalThis.__SMOKE_INSTALLING_TEXT__.includes('close and reopen')
        || target.textContent.includes('close and reopen')) {
        throw new Error('Linux claimed an automatic relaunch it does not perform');
      }
      if (!button('Restart now')) throw new Error('a completed Linux install offered no Restart now');
      if (!button('Later')) throw new Error('a completed Linux install offered no Later');
      button('Restart now').click();
      await settle(flushSync);
      if (!globalThis.__SMOKE_RESTARTED__) throw new Error('Restart now did not reach restart_app');
    },
  },
  // Later dismisses the prompt without restarting: the new version is already
  // on disk, so the only thing left to decide is when it starts.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    update: {
      available: true, current: '0.1.0', latest: '0.2.0', notes: 'n',
      pub_date: '2026-08-05T09:21:10Z', url: 'https://example.test/app.AppImage',
      installable: true, restart_after_install: true,
    },
    verify: async ({ target, flushSync }) => {
      const button = (label) => [...target.querySelectorAll('button')]
        .find((b) => b.textContent.trim() === label);
      button('Install update').click();
      await settle(flushSync);
      button('Later').click();
      flushSync();
      if (globalThis.__SMOKE_RESTARTED__) throw new Error('Later restarted the app anyway');
      if (button('Restart now')) throw new Error('Later left the restart prompt on screen');
      if (!target.textContent.includes('next time you open the app')) {
        throw new Error('Later did not say when the new version starts');
      }
    },
  },
  // Nix: a release exists, and on Linux it even publishes an AppImage, but a
  // package-managed build is not an installable artifact. Guidance, not an
  // Install action that would fail.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    update: {
      available: true, current: '0.1.0', latest: '0.2.0', notes: 'n',
      pub_date: '2026-08-05T09:21:10Z', url: null,
      installable: false, restart_after_install: false,
    },
    expect: ['Update available: v0.2.0', 'nix profile upgrade quota-widget'],
    verify: async ({ target }) => {
      const install = [...target.querySelectorAll('button')]
        .find((b) => b.textContent.trim() === 'Install update');
      if (install) throw new Error('offered Install for a release with no artifact for this build');
    },
  },
  // A portable EXE sees the same Windows installer URL as an installed build,
  // but Tauri reports no bundle type for it. It must get the upgrade guidance,
  // never an Install action that cannot replace the running executable.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    update: {
      available: true, current: '0.1.0', latest: '0.2.0', notes: 'n',
      pub_date: '2026-08-05T09:21:10Z', url: 'https://example.test/setup.exe',
      installable: false, restart_after_install: false,
    },
    expect: ['Update available: v0.2.0', 'nix profile upgrade quota-widget'],
    verify: async ({ target }) => {
      const install = [...target.querySelectorAll('button')]
        .find((b) => b.textContent.trim() === 'Install update');
      if (install) throw new Error('offered Install to a portable executable');
    },
  },
  // Desktop integration in Settings. Absent from every build that has no
  // launcher to manage — the Windows installer and the Nix package own theirs.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    verify: ({ target }) => {
      if (target.querySelector('.desktop-integration')) {
        throw new Error('offered launcher controls to a build with no launcher to manage');
      }
    },
  },
  // An AppImage with no launcher offers to add one, and only that.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    desktop: { state: 'absent', appimage: '/home/u/Downloads/q.AppImage', desktop_file: '/home/u/.local/share/applications/quota-widget.desktop' },
    expect: ['Applications menu', 'Add to applications menu'],
    verify: async ({ target, flushSync }) => {
      const button = (text) => [...target.querySelectorAll('.desktop-integration button')]
        .find((b) => b.textContent.trim() === text);
      if (button('Remove from applications menu')) {
        throw new Error('offered to remove a launcher that does not exist');
      }
      button('Add to applications menu').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      flushSync();
      if (!globalThis.__SMOKE_DESKTOP_CALLS__.includes('add')) {
        throw new Error('Add did not reach desktop_integration_add');
      }
    },
  },
  // An installed launcher offers removal, and removal that keeps a file the
  // user edited has to say so rather than claiming a clean removal.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    desktop: { state: 'current', appimage: '/home/u/Downloads/q.AppImage', desktop_file: '/home/u/.local/share/applications/quota-widget.desktop' },
    desktopRemove: { removed: ['/home/u/.local/share/applications/quota-widget.desktop'], preserved: ['/home/u/.local/share/icons/hicolor/32x32/apps/quota-widget.png'] },
    expect: ['Remove from applications menu'],
    verify: async ({ target, flushSync }) => {
      const button = (text) => [...target.querySelectorAll('.desktop-integration button')]
        .find((b) => b.textContent.trim() === text);
      if (button('Add to applications menu')) throw new Error('offered to add a launcher that is already current');
      button('Remove from applications menu').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      flushSync();
      if (!globalThis.__SMOKE_DESKTOP_CALLS__.includes('remove')) {
        throw new Error('Remove did not reach desktop_integration_remove');
      }
      const message = target.querySelector('.desktop-message')?.textContent ?? '';
      if (!message.includes('quota-widget.png') || !/by hand/.test(message)) {
        throw new Error(`removal did not explain the manual cleanup: ${message}`);
      }
    },
  },
  // A moved AppImage offers repair, names both paths, and does not retarget
  // until the button is pressed.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    desktop: { state: 'stale', target: '/home/u/old/q.AppImage', appimage: '/home/u/Downloads/q.AppImage', desktop_file: '/home/u/.local/share/applications/quota-widget.desktop' },
    desktopAdd: { action: 'repaired', written: [], preserved: [] },
    expect: ['Repair launcher', '/home/u/old/q.AppImage', '/home/u/Downloads/q.AppImage'],
    verify: async ({ target, flushSync }) => {
      const button = (text) => [...target.querySelectorAll('.desktop-integration button')]
        .find((b) => b.textContent.trim() === text);
      if (globalThis.__SMOKE_DESKTOP_CALLS__.length) {
        throw new Error('a moved AppImage was retargeted just by opening Settings');
      }
      button('Repair launcher').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      flushSync();
      if (!globalThis.__SMOKE_DESKTOP_CALLS__.includes('add')) throw new Error('Repair did not reach the add command');
      if (!/repaired/i.test(target.querySelector('.desktop-message')?.textContent ?? '')) {
        throw new Error('repair did not report what it did');
      }
    },
  },
  // The deferral is Rust's to own. Settings edits a snapshot taken at mount,
  // so a config that predates the field must save a real boolean rather than
  // undefined — otherwise saving Settings would make the prompt return.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => {
      const old = structuredClone(CONFIG);
      delete old.desktop_integration_prompted;
      return { initialConfig: $.proxy(old), snapshots: structuredClone(SNAPSHOTS), onclose() {} };
    },
    verify: async ({ target }) => {
      target.querySelector('.primary').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      if (globalThis.__SMOKE_LAST_CONFIG__?.desktop_integration_prompted !== false) {
        throw new Error(`saved deferral flag was ${globalThis.__SMOKE_LAST_CONFIG__?.desktop_integration_prompted}`);
      }
    },
  },
  // A launcher the user owns is never offered an action that would overwrite
  // or delete it — only an explanation of where it is.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    desktop: { state: 'user_owned', appimage: '/home/u/Downloads/q.AppImage', desktop_file: '/home/u/.local/share/applications/quota-widget.desktop' },
    expect: ['Applications menu', '/home/u/.local/share/applications/quota-widget.desktop'],
    verify: ({ target }) => {
      if (target.querySelectorAll('.desktop-integration button').length) {
        throw new Error('offered an action over a launcher the user owns');
      }
    },
  },
  {
    file: 'src/lib/shared/UsageCard.svelte',
    props: () => ({ snap: structuredClone(SNAPSHOTS[0]) }),
    verify: async ({ target }) => {
      const bars = [...target.querySelectorAll('.bar')];
      if (bars.length !== 2) throw new Error(`expected 2 bars, got ${bars.length}`);
      // The 5-hour window is one hour into five, so the marker sits at 20%.
      const mark = bars[0].querySelector('.period-mark');
      if (!mark) throw new Error('bounded window rendered no period marker');
      const pct = parseFloat(mark.style.left);
      if (!(Math.abs(pct - 20) < 1)) throw new Error(`marker sat at ${mark.style.left}, expected ~20%`);
      // The weekly window has no period_start, so it gets no marker at all
      // rather than one pinned at an end.
      if (bars[1].querySelector('.period-mark')) {
        throw new Error('window without a period start still drew a marker');
      }
    },
  },
  // Spend with no budget configured: the label must prefix the amount, so the
  // figure cannot be misread as money remaining. The unlabelled balance above
  // it must stay bare.
  {
    file: 'src/lib/shared/UsageCard.svelte',
    props: () => ({ snap: structuredClone(SNAPSHOTS[4]) }),
    expect: ['Cost this month: 8.75 USD'],
  },
  {
    file: 'src/lib/shared/UsageCard.svelte',
    props: () => ({ snap: structuredClone(SNAPSHOTS[2]) }),
    verify: ({ target }) => {
      const balance = target.querySelector('.balance').textContent.trim();
      if (balance !== '3.42 USD') throw new Error(`unlabelled balance rendered as ${balance}`);
    },
  },
  // Thresholds: the shared warn/critical percent editor, reflecting whatever
  // object it is handed.
  {
    file: 'src/lib/shared/Thresholds.svelte',
    props: () => ({ thresholds: { warn_pct: 80, critical_pct: 95 } }),
    expect: ['Warn at', 'Critical at'],
    verify: ({ target }) => {
      const nums = [...target.querySelectorAll('input.num')].map((el) => el.value);
      if (nums.join(',') !== '80,95') throw new Error(`thresholds rendered as ${nums.join(',')}`);
    },
  },
  // Ordering: the basis select stays inert while the order is Manual, exactly
  // as it does inline in Settings.
  {
    file: 'src/lib/shared/Ordering.svelte',
    props: () => ({ sortOrder: 'manual', sortBasis: 'icon' }),
    expect: ['Order accounts by', 'Sorting on'],
    verify: ({ target }) => {
      const basisSelect = [...target.querySelectorAll('select')].find((el) => el.value === 'icon');
      if (!basisSelect.disabled) throw new Error('basis select was live under Manual');
    },
  },
  // HeadlineSelection: opening the menu and toggling an item updates the
  // summary text and the account's saved selection, the same behaviour
  // Settings.svelte exercises inline.
  {
    file: 'src/lib/shared/HeadlineSelection.svelte',
    props: () => ({
      account: { mini_summary_metrics: ['credits'], tray_metric: null, enabled: true },
      options: [{ id: 'credits', label: 'Credit balance' }, { id: 'window:monthly_spend', label: 'Monthly' }],
      open: false,
      onToggle() { globalThis.__SMOKE_TOGGLED__ = true; },
    }),
    expect: ['Mini-summary headlines', 'Tray icon status'],
    verify: ({ target }) => {
      const toggle = target.querySelector('.metric-toggle');
      if (toggle.textContent.trim() !== 'Credit balance ▾') {
        throw new Error(`summary text was ${toggle.textContent.trim()}`);
      }
      toggle.click();
      if (!globalThis.__SMOKE_TOGGLED__) throw new Error('clicking the toggle did not call onToggle');
    },
  },
  // SimpleKeyAccount: a plain API-key account row — collapsed by default,
  // expands to show the headline picker and secret input, and reaches its
  // callbacks on Test/Remove.
  {
    file: 'src/lib/shared/SimpleKeyAccount.svelte',
    props: () => ({
      id: 'openrouter',
      kind: 'openrouter',
      account: { label: null, enabled: true, low_balance_warn: null, mini_summary_metrics: null, tray_metric: null, settings: {} },
      providerName: 'OpenRouter',
      providerNote: 'Create a key at openrouter.ai/keys.',
      secretStored: false,
      secretInput: '',
      headlineOptions: [{ id: 'credits', label: 'Credit balance' }],
      headlineOpen: false,
      onToggleHeadline() {},
      expanded: false,
      testResult: null,
      onTest() { globalThis.__SMOKE_TESTED__ = true; },
      onClearSecret() {},
      onRemove() { globalThis.__SMOKE_REMOVED__ = true; },
    }),
    verify: ({ target, flushSync }) => {
      if (target.querySelector('.metric-picker')) throw new Error('account did not start collapsed');
      target.querySelector('.provider-disclosure').click();
      flushSync();
      if (!target.querySelector('.metric-picker')) throw new Error('expanding did not reveal the headline picker');
      if (!target.querySelector('input[type="password"]')) throw new Error('no secret input was rendered');
      [...target.querySelectorAll('.provider-footer button')].find((b) => b.textContent.trim() === 'Test').click();
      if (!globalThis.__SMOKE_TESTED__) throw new Error('Test did not reach onTest');
      [...target.querySelectorAll('.provider-footer button')].find((b) => b.textContent.trim() === 'Remove account').click();
      if (!globalThis.__SMOKE_REMOVED__) throw new Error('Remove did not reach onRemove');
    },
  },
  // MobileApp: the Android shell. Opens directly to the usage list (no
  // window/tray chrome); Settings doubles as onboarding when the account
  // list is empty (issue #109 — a true first run has no accounts at all,
  // see `Config::mobile_first_run_default`).
  {
    file: 'src/mobile/MobileApp.svelte',
    props: () => ({}),
    config: { ...CONFIG, providers: {} },
    verify: async ({ target, flushSync }) => {
      if (target.querySelector('.mobile-header button[title="Back"]')) {
        throw new Error('the list view showed a Back button');
      }
      target.querySelector('button[title="Settings"]').click();
      flushSync();
      if (!target.querySelector('.add-account-toggle')) {
        throw new Error('an empty account list did not offer to add one (onboarding)');
      }
      target.querySelector('.add-account-toggle').click();
      flushSync();
      const kindSelect = target.querySelector('.add-account select');
      if (!kindSelect || kindSelect.options.length < 5) {
        throw new Error('the provider picker did not list the direct-HTTPS providers');
      }
      [...target.querySelectorAll('.add-account-actions button')]
        .find((b) => b.textContent.trim() === 'Add account')
        .click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      flushSync();
      if (!target.querySelector('.provider')) throw new Error('adding an account did not render its row');
      if (globalThis.__SMOKE_LAST_CONFIG__?.providers?.['openrouter#1']?.kind !== 'openrouter') {
        throw new Error('adding an account did not persist an openrouter entry under an immutable key');
      }
    },
  },
];

function stubTauri() {
  mkdirSync(join(WORK, '@tauri-apps/api'), { recursive: true });
  const w = (p, s) => writeFileSync(join(WORK, p), s);
  w('@tauri-apps/api/core.js', `
export async function invoke(cmd, args) {
  switch (cmd) {
    case 'get_snapshots': if (globalThis.__SMOKE_SNAPSHOTS_ERROR__) throw new Error('IPC unavailable'); return { ...(globalThis.__SMOKE_CONFIG__ ?? ${JSON.stringify({ snapshots: SNAPSHOTS, config: CONFIG })}), config_recovery: globalThis.__SMOKE_RECOVERY__ ?? null };
    case 'app_version': return '0.0.0-test';
    case 'has_secret': return false;
    case 'on_wayland': return true;
    case 'set_config':
      if (globalThis.__SMOKE_SAVE_ERROR__) throw new Error(globalThis.__SMOKE_SAVE_ERROR__);
      globalThis.__SMOKE_LAST_CONFIG__ = args.config; return null;
    // The only command allowed to displace an unreadable config, so a case
    // that never clicks the button must never see it called.
    case 'recover_config':
      if (globalThis.__SMOKE_RECOVER_ERROR__) throw new Error(globalThis.__SMOKE_RECOVER_ERROR__);
      (globalThis.__SMOKE_SHELL_CALLS__ ??= []).push('recover_config');
      globalThis.__SMOKE_LAST_CONFIG__ = args.config;
      return globalThis.__SMOKE_RECOVERY_KEPT__ ?? null;
    case 'exit_settings': (globalThis.__SMOKE_SHELL_CALLS__ ??= []).push(\`exit_settings:\${args.to}\`); return null;
    case 'hide_window': (globalThis.__SMOKE_SHELL_CALLS__ ??= []).push('hide_window'); return null;
    case 'update_status': case 'check_update_now': return globalThis.__SMOKE_UPDATE__ ?? null;
    // Desktop integration. A non-AppImage build answers null, which is what
    // keeps the section and the first-run prompt out of every other build.
    case 'desktop_integration_status': return globalThis.__SMOKE_DESKTOP__ ?? null;
    case 'desktop_integration_add':
      if (globalThis.__SMOKE_DESKTOP_ERROR__) throw new Error(globalThis.__SMOKE_DESKTOP_ERROR__);
      globalThis.__SMOKE_DESKTOP_CALLS__.push('add');
      return globalThis.__SMOKE_DESKTOP_ADD__ ?? { action: 'created', written: [], preserved: [] };
    case 'desktop_integration_remove':
      globalThis.__SMOKE_DESKTOP_CALLS__.push('remove');
      return globalThis.__SMOKE_DESKTOP_REMOVE__ ?? { removed: [], preserved: [] };
    case 'mark_desktop_integration_prompted':
      globalThis.__SMOKE_DESKTOP_CALLS__.push('prompted'); return null;
    case 'restart_app': globalThis.__SMOKE_RESTARTED__ = true; return null;
    default: return null;
  }
}`);
  // Listeners are retained so a case can play back the events Rust really
  // emits: 'mini-shown', whose whole contract is that it does *not* disturb the
  // summary's fade level, and the tray's 'navigate' payload, which is the only
  // way the Settings return state crosses the navigation boundary. The registry
  // is per case; the runner clears it between mounts so one component's
  // listeners can never be fired by the next one's events.
  w('@tauri-apps/api/event.js', `
export async function listen(event, handler) {
  const listeners = (globalThis.__SMOKE_LISTENERS__ ??= new Map());
  const forEvent = listeners.get(event) ?? [];
  forEvent.push(handler);
  listeners.set(event, forEvent);
  return () => {
    const current = listeners.get(event) ?? [];
    const at = current.indexOf(handler);
    if (at >= 0) current.splice(at, 1);
  };
}`);
  // Settings imports this lazily when Install update is pressed. Stubbed so the
  // button can be exercised without a real updater or a real download.
  mkdirSync(join(WORK, '@tauri-apps/plugin-updater'), { recursive: true });
  w('@tauri-apps/plugin-updater/index.js', `
export async function check() {
  if (globalThis.__SMOKE_UPDATE_CHECK_NULL__) return null;
  return {
    downloadAndInstall: async () => {
      // Yield first, so Svelte flushes the message shown *while* the install
      // runs. That message is a contract of its own — it is the last thing a
      // Windows user sees before the app exits — and the completion message
      // overwrites it, so a check made afterwards cannot see it.
      await new Promise((resolve) => setTimeout(resolve, 0));
      globalThis.__SMOKE_INSTALLING_TEXT__ = globalThis.document.body.textContent;
      globalThis.__SMOKE_INSTALLED__ = true;
    },
  };
}`);
  // Every size the window is asked for is recorded, so a case can assert on
  // what the view swap did to the window rather than on the helper that did it.
  // "Settings kept the usage height" is only observable as the *absence* of a
  // request, which needs the whole sequence, not just the latest value.
  w('@tauri-apps/api/window.js', `
export function getCurrentWindow() {
  return {
    setSize: async (size) => { (globalThis.__SMOKE_SIZES__ ??= []).push({ width: size.width, height: size.height }); },
    outerPosition: async () => ({x:0,y:0}),
    scaleFactor: async () => 1,
  };
}`);
  w('@tauri-apps/api/dpi.js', `
export class LogicalSize { constructor(w,h){ this.width=w; this.height=h; } }
export class LogicalPosition { constructor(x,y){ this.x=x; this.y=y; } }`);
}

// Rewrite bare `@tauri-apps/...` specifiers to the stubs above, relative to
// where `rel` will land in WORK. Shared between compiled .svelte output and
// plain .js modules (like lib/host.js) that import Tauri directly — without
// this a plain-.js copy would resolve the *real* npm package instead of the
// stub, and its calls to `window.__TAURI_INTERNALS__` would throw under jsdom.
function rewriteTauriImports(code, rel) {
  code = code.replace(/(from\s+')@tauri-apps\/api\/([\w-]+)(')/g, (_m, a, mod, z) => {
    const target = relative(dirname(rel), `@tauri-apps/api/${mod}.js`);
    return `${a}${target.startsWith('.') ? target : './' + target}${z}`;
  });
  // Settings pulls the updater in with a dynamic import() rather than a static
  // `from`, so it needs its own rewrite or the specifier reaches Node bare.
  code = code.replace(/(import\(\s*')@tauri-apps\/([\w-]+)('\s*\))/g, (_m, a, pkg, z) => {
    const target = relative(dirname(rel), `@tauri-apps/${pkg}/index.js`);
    return `${a}${target.startsWith('.') ? target : './' + target}${z}`;
  });
  return code;
}

// Compile a component and its local .svelte imports, rewriting bare Tauri
// specifiers to the stubs above.
const built = new Set();
function build(rel) {
  if (built.has(rel)) return join(WORK, rel.replace(/\.svelte$/, '.js'));
  built.add(rel);
  const src = readFileSync(join(ROOT, rel), 'utf8');
  const { js } = compile(src, { generate: 'client', filename: rel });
  let code = rewriteTauriImports(js.code, rel);
  // Recurse into sibling component imports so App pulls in its children.
  for (const m of code.matchAll(/from\s+'(\.[^']*\.svelte)'/g)) {
    build(join(dirname(rel), m[1]).replace(/^\/+/, ''));
  }
  code = code.replace(/(from\s+'\.[^']*)\.svelte'/g, "$1.js'");
  const out = join(WORK, rel.replace(/\.svelte$/, '.js'));
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, code);
  return out;
}

const dom = new JSDOM('<!doctype html><div id="app"></div>', { pretendToBeVisual: true });
for (const k of ['window', 'document', 'HTMLElement', 'Element', 'Node', 'Event',
  'CustomEvent', 'requestAnimationFrame', 'cancelAnimationFrame', 'getComputedStyle',
  'MutationObserver', 'SVGElement', 'HTMLMediaElement', 'Text', 'Comment', 'DocumentFragment']) {
  globalThis[k] = dom.window[k];
}
globalThis.__QUOTA_WIDGET_VERSION__ = '0.0.0-test';
globalThis.__QUOTA_WIDGET_BRANCH__ = '';

rmSync(WORK, { recursive: true, force: true });
stubTauri();
mkdirSync(join(WORK, 'src/lib'), { recursive: true });
// Every plain module in src/lib, not a named list: a component importing a
// helper this harness forgot to stage fails to render for a reason that has
// nothing to do with the component.
for (const f of readdirSync(join(ROOT, 'src/lib')).filter((f) => f.endsWith('.js'))) {
  const rel = `src/lib/${f}`;
  const code = rewriteTauriImports(readFileSync(join(ROOT, rel), 'utf8'), rel);
  writeFileSync(join(WORK, rel), code);
}

const { mount, unmount, flushSync } = await import('svelte');
const $ = await import('svelte/internal/client');

// Plays back an event exactly as Rust's `app.emit` would, for the listeners
// the component registered in `onMount`.
//
// Handlers are invoked synchronously and the result flushed before anything is
// awaited, so a case that does not await this still sees the render — a handler
// that only assigns state (`navigate` swapping the view) has already done its
// work by then. Awaiting additionally waits out handlers that go async.
function emit(event, payload) {
  const pending = [...(globalThis.__SMOKE_LISTENERS__?.get(event) ?? [])]
    .map((handler) => handler({ event, payload }));
  flushSync();
  return Promise.all(pending);
}

let failed = 0;
for (const c of CASES) {
  globalThis.__SMOKE_LISTENERS__ = new Map();
  const target = dom.window.document.createElement('div');
  dom.window.document.body.appendChild(target);
  let app;
  try {
    globalThis.__SMOKE_SNAPSHOTS_ERROR__ = Boolean(c.snapshotsError);
    globalThis.__SMOKE_CONFIG__ = c.config ? { snapshots: SNAPSHOTS, config: c.config } : undefined;
    globalThis.__QUOTA_WIDGET_BRANCH__ = c.buildBranch ?? '';
    globalThis.__SMOKE_UPDATE__ = c.update ?? null;
    globalThis.__SMOKE_SAVE_ERROR__ = c.saveError ?? '';
    globalThis.__SMOKE_INSTALLED__ = false;
    globalThis.__SMOKE_DESKTOP__ = c.desktop ?? null;
    globalThis.__SMOKE_DESKTOP_ADD__ = c.desktopAdd ?? null;
    globalThis.__SMOKE_DESKTOP_REMOVE__ = c.desktopRemove ?? null;
    globalThis.__SMOKE_DESKTOP_ERROR__ = c.desktopError ?? '';
    globalThis.__SMOKE_DESKTOP_CALLS__ = [];
    globalThis.__SMOKE_RECOVERY__ = c.configRecovery ?? null;
    globalThis.__SMOKE_RECOVERY_KEPT__ = c.recoveryKept ?? null;
    globalThis.__SMOKE_RECOVER_ERROR__ = c.recoverError ?? '';
    globalThis.__SMOKE_RESTARTED__ = false;
    globalThis.__SMOKE_INSTALLING_TEXT__ = '';
    globalThis.__SMOKE_SHELL_CALLS__ = [];
    globalThis.__SMOKE_SIZES__ = [];
    app = mount((await import(build(c.file))).default, { target, props: c.props($) });
    flushSync();
    await new Promise((r) => setTimeout(r, 60)); // let onMount's awaits settle
    flushSync();
    // `emit` plays a Rust event to the component's own listeners; `shellCalls`
    // is what it asked Rust to do in return. Together they cover the navigation
    // seam in both directions without reaching into component internals.
    const shellCalls = () => globalThis.__SMOKE_SHELL_CALLS__;
    const sizes = () => globalThis.__SMOKE_SIZES__;
    await c.verify?.({ target, flushSync, emit, shellCalls, sizes });
  } catch (e) {
    console.error(`FAIL ${c.file}\n      ${String(e.message).split('\n')[0]}`);
    failed++;
    continue;
  }
  const html = target.innerHTML;
  const missing = (c.expect ?? []).filter((t) => !html.includes(t));
  if (!html.trim()) {
    console.error(`FAIL ${c.file}\n      mounted but rendered nothing`);
    failed++;
  } else if (missing.length) {
    console.error(`FAIL ${c.file}\n      rendered but missing: ${missing.join(', ')}`);
    failed++;
  } else {
    console.log(`ok   ${c.file} (${html.length} bytes)`);
  }
  try { unmount(app); } catch {}
  globalThis.__SMOKE_SNAPSHOTS_ERROR__ = false;
  globalThis.__SMOKE_CONFIG__ = undefined;
  globalThis.__SMOKE_UPDATE__ = null;
  globalThis.__SMOKE_SAVE_ERROR__ = '';
  globalThis.__SMOKE_DESKTOP__ = null;
  target.remove();
}

rmSync(WORK, { recursive: true, force: true });
if (failed) {
  console.error(`\nsmoke-mount: ${failed} component(s) failed to render`);
  process.exit(1);
}
console.log('\nsmoke-mount: all components mounted and rendered');
