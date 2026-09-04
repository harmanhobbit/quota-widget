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
    // All seven active: identical to the field being absent, which is what a
    // config predating the schedule serializes. Keep in sync with
    // crates/quota-core/src/config.rs `UsageSchedule::default`.
    usage_schedule: { monday: true, tuesday: true, wednesday: true, thursday: true, friday: true, saturday: true, sunday: true },
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
    provider_name: 'Nous',
    error: null,
    credits: { balance: 12, unit: 'USD' },
    windows: [{ metric_id: 'monthly_allowance', label: 'Monthly allowance (Plus)', used_pct: 100, informational: true }],
  },
  // Z.ai (issue #183): percentage-only token windows plus an informational
  // monthly web/tool window with its absolute counts.
  {
    provider_id: 'zai',
    provider_name: 'Z.ai',
    error: null,
    credits: null,
    windows: [
      { metric_id: 'five_hour', label: '5-hour', used_pct: 8, informational: false },
      { metric_id: 'weekly', label: 'Weekly', used_pct: 52, informational: false },
      {
        metric_id: 'web_tool_month',
        label: 'Web/tool this month',
        used_pct: 25,
        informational: true,
        allowance: { remaining: 75, total: 100, unit: 'calls' },
      },
    ],
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

// Reveal a top-level Settings disclosure (issue #184) so a case can drive the
// controls that now start collapsed behind it. Idempotent — a section already
// open is left open. Scoped to the section headers (`.disclosure > h2`) so it
// never matches a provider-account row, which shares the button styling. This
// is the externally visible operation the feature adds: activating the heading
// button, whose aria-expanded the panel's visibility must track.
function openSection(target, id, flushSync) {
  const btn = target.querySelector(`.disclosure > h2 > button#${id}-toggle`);
  if (!btn) throw new Error(`no top-level section "${id}" to open`);
  if (btn.getAttribute('aria-controls') !== `${id}-panel`) {
    throw new Error(`section "${id}" toggle does not control its panel`);
  }
  if (btn.getAttribute('aria-expanded') !== 'true') {
    btn.click();
    flushSync();
  }
  if (!target.querySelector(`#${id}-panel`)) {
    throw new Error(`opening section "${id}" did not reveal its panel`);
  }
  return btn;
}

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

// A weekly window one day into a 7-day period, anchored to now so the calendar
// marker sits at ~1/7 regardless of when the suite runs. Used by the
// usage-schedule marker cases below.
function weeklyWindowOneDayIn() {
  const now = Date.now();
  return {
    metric_id: 'weekly',
    label: 'Weekly',
    used_pct: 50,
    informational: false,
    period_start: new Date(now - 24 * 60 * 60_000).toISOString(),
    resets_at: new Date(now + 6 * 24 * 60 * 60_000).toISOString(),
  };
}
// getDay() is Sunday 0 … Saturday 6; the schedule keys are weekday names.
const SCHEDULE_DAY_KEYS = ['sunday', 'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday'];
function allDaysSchedule() {
  return Object.fromEntries(SCHEDULE_DAY_KEYS.map((k) => [k, true]));
}
// Deactivate the two weekdays the elapsed portion [start, now] falls across,
// so no scheduled time has elapsed yet and the scheduled marker sits at 0%.
function elapsedDaysOffSchedule() {
  const s = allDaysSchedule();
  const now = Date.now();
  s[SCHEDULE_DAY_KEYS[new Date(now).getDay()]] = false;
  s[SCHEDULE_DAY_KEYS[new Date(now - 24 * 60 * 60_000).getDay()]] = false;
  return s;
}

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
        // Thresholds starts collapsed each visit (issue #184), so the warn
        // field lives behind its disclosure until the heading is activated.
        openSection(target, 'thresholds', flushSync);
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
        // The reopened visit starts with Thresholds collapsed again — its
        // disclosure reset — so reveal it before reading the persisted value.
        openSection(target, 'thresholds', flushSync);
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
  // Issue #176: both windows start hidden, and a window GTK has not mapped
  // yet reports its DOM width as 0. A content fit that forwards that width is
  // the `gtk_window_resize` assertion 'width > 0' printed at every Linux
  // start. With the DOM forced to the zero width a hidden window reports, no
  // resize request may carry a non-positive width — and once the window is
  // visible again, the same effect must fit for real, so the guard is a
  // deferral rather than a switch-off.
  {
    file: 'src/App.svelte',
    props: () => ({}),
    verify: async ({ flushSync, emit, sizes }) => {
      await settle(flushSync);
      const fitted = sizes().at(-1);
      if (!fitted || fitted.width <= 0) {
        throw new Error('the visible popup never fitted its content');
      }
      const visible = Object.getOwnPropertyDescriptor(window, 'innerWidth');
      Object.defineProperty(window, 'innerWidth', { configurable: true, value: 0 });
      try {
        // The trigger that fires at startup: a snapshot push re-runs the fit
        // effect while the window is still unmapped. A fresh array, because a
        // poll's payload is a new one even when the accounts have not changed.
        sizes().length = 0;
        emit('snapshots', [...SNAPSHOTS]);
        await settle(flushSync);
        const bad = sizes().filter((s) => !(s.width > 0));
        if (bad.length) {
          throw new Error(`a zero-width fit still requested ${JSON.stringify(bad[0])}`);
        }
        // The window is visible again: content fitting must come back.
        Object.defineProperty(window, 'innerWidth', visible);
        sizes().length = 0;
        emit('snapshots', [...SNAPSHOTS]);
        await settle(flushSync);
        const refit = sizes().at(-1);
        if (!refit || refit.width <= 0) {
          throw new Error(`a visible popup stopped fitting after the guard: ${JSON.stringify(refit)}`);
        }
      } finally {
        Object.defineProperty(window, 'innerWidth', visible);
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
      // Every top-level section now starts collapsed each visit (issue #184):
      // no account card, and no General control, is on screen until its
      // heading is activated. That the panels appear only after opening is the
      // feature's core behaviour.
      if (target.querySelector('.provider')) throw new Error('Providers did not start collapsed');
      if (target.textContent.includes('Fade windows when scrolling over them')) {
        throw new Error('General did not start collapsed');
      }
      openSection(target, 'providers', flushSync);
      openSection(target, 'general', flushSync);
      // Accounts keep their own independent disclosure inside Providers, and
      // still start collapsed — revealing the section does not expand them.
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
  // Z.ai joins the normal provider flow (issue #183): the desktop selector
  // offers it, adding one creates an account under an immutable `zai#1` key
  // with the zai kind and a custom label, its row exposes a pasted-API-key
  // secret field like every other key provider, and Save persists the entry
  // through the ordinary set_config path.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    expect: ['Z.ai'],
    verify: async ({ target, flushSync }) => {
      const findButton = (text) => [...target.querySelectorAll('button')].find((button) => button.textContent.trim() === text);
      openSection(target, 'providers', flushSync);
      findButton('+ Add account').click();
      flushSync();
      const nameInput = target.querySelector('.add-account input');
      if (!nameInput) throw new Error('add-account did not offer a name field');
      nameInput.value = 'Personal Z.ai';
      nameInput.dispatchEvent(new window.Event('input', { bubbles: true }));
      const kindSelect = target.querySelector('.add-account select');
      const zaiOption = [...kindSelect.options].find((o) => o.value === 'zai');
      if (!zaiOption || zaiOption.textContent !== 'Z.ai') {
        throw new Error(`the provider picker did not offer Z.ai: ${[...kindSelect.options].map((o) => o.value).join(',')}`);
      }
      kindSelect.value = 'zai';
      kindSelect.dispatchEvent(new window.Event('change', { bubbles: true }));
      flushSync();
      findButton('Add account').click();
      flushSync();
      // The new row opens automatically and must expose the key paste field
      // through the shared pasted-key path.
      const card = [...target.querySelectorAll('.provider')]
        .find((el) => el.querySelector('strong').textContent.includes('Personal Z.ai'));
      if (!card) throw new Error('the added Z.ai account did not render a row');
      const secretInput = card.querySelector('input[type="password"]');
      if (!secretInput || !secretInput.placeholder.includes('API key')) {
        throw new Error('the Z.ai row did not offer the pasted API key field');
      }
      target.querySelector('.primary').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      flushSync();
      const saved = globalThis.__SMOKE_LAST_CONFIG__;
      const entry = saved?.providers?.['zai#1'];
      if (!entry || entry.kind !== 'zai' || entry.label !== 'Personal Z.ai' || !entry.enabled) {
        throw new Error(`Save did not persist the Z.ai account correctly: ${JSON.stringify(saved?.providers?.['zai#1'])}`);
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
  // The per-account usage schedule editor: seven weekday toggles, all on by
  // default, and unchecking one persists through Save. A config that predates
  // the field (no usage_schedule at all) must load with all seven on rather
  // than leaving the toggles unset, mirroring the Rust serde default.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => {
      const cfg = structuredClone(CONFIG);
      // Simulate an upgrade: the field is entirely absent from an old file.
      delete cfg.providers.claude.usage_schedule;
      return { initialConfig: $.proxy(cfg), snapshots: structuredClone(SNAPSHOTS), onclose() {} };
    },
    expect: ['Usage days'],
    verify: async ({ target, flushSync }) => {
      openSection(target, 'providers', flushSync);
      const card = (name) => [...target.querySelectorAll('.provider')]
        .find((el) => el.querySelector('strong').textContent.trim() === name);
      card('Claude').querySelector('.provider-disclosure').click();
      flushSync();
      const toggles = [...card('Claude').querySelectorAll('.schedule-day input')];
      if (toggles.length !== 7) throw new Error(`schedule editor had ${toggles.length} toggles, expected 7`);
      // An absent field loads as all-seven, not as seven unset checkboxes.
      if (!toggles.every((t) => t.checked)) {
        throw new Error('an absent usage_schedule did not default to all seven on');
      }
      toggles[0].click();
      flushSync();
      if (toggles[0].checked) throw new Error('clicking a schedule toggle did not uncheck it');
      target.querySelector('.primary').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      const saved = globalThis.__SMOKE_LAST_CONFIG__;
      const keys = ['monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday', 'sunday'];
      const vals = keys.map((k) => saved?.providers?.claude?.usage_schedule?.[k]);
      // The first toggle is Mon; the rest stay on. All seven keys must save.
      if (vals.length !== 7 || vals[0] !== false || !vals.slice(1).every((v) => v === true)) {
        throw new Error(`saved schedule was ${JSON.stringify(vals)}`);
      }
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
      openSection(target, 'general', flushSync);
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
      openSection(target, 'general', flushSync);
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
      openSection(target, 'general', flushSync);
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
      openSection(target, 'general', flushSync);
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
      openSection(target, 'general', flushSync);
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
    verify: async ({ target, flushSync }) => {
      openSection(target, 'general', flushSync);
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
    verify: async ({ target, flushSync }) => {
      openSection(target, 'general', flushSync);
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
      openSection(target, 'desktop', flushSync);
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
      openSection(target, 'desktop', flushSync);
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
      openSection(target, 'desktop', flushSync);
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
    verify: ({ target, flushSync }) => {
      openSection(target, 'desktop', flushSync);
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
  // The usage schedule reshapes a weekly window's marker. Two cases share one
  // window anchored to now — a 7-day period one day in — so the calendar marker
  // sits at ~1/7. With all seven days active the scheduled marker is the
  // calendar marker (criterion: all-seven renders exactly as before). With the
  // two weekdays the elapsed portion falls on deactivated, the scheduled
  // marker freezes at 0% — proving the schedule reaches the computation and
  // changes the result, without hardcoding a brittle fraction. The precise
  // scheduled math is covered by period.test.js; this proves the wiring.
  // getDay() is Sunday 0 … Saturday 6; the schedule keys are weekday names.
  {
    file: 'src/lib/shared/UsageCard.svelte',
    props: () => ({ snap: { provider_name: 'Claude', fetched_at: new Date().toISOString(), error: null, credits: null, windows: [weeklyWindowOneDayIn()] }, schedule: allDaysSchedule() }),
    verify: ({ target }) => {
      const mark = target.querySelector('.bar .period-mark');
      if (!mark) throw new Error('weekly window with an all-seven schedule drew no marker');
      const pct = parseFloat(mark.style.left);
      // 1 day into a 7-day window → ~14.3%. The marker is the calendar fraction.
      if (!(pct > 13 && pct < 16)) throw new Error(`all-seven schedule marker sat at ${mark.style.left}, expected ~14%`);
    },
  },
  {
    file: 'src/lib/shared/UsageCard.svelte',
    props: () => ({ snap: { provider_name: 'Claude', fetched_at: new Date().toISOString(), error: null, credits: null, windows: [weeklyWindowOneDayIn()] }, schedule: elapsedDaysOffSchedule() }),
    verify: ({ target }) => {
      const mark = target.querySelector('.bar .period-mark');
      if (!mark) throw new Error('weekly window with a schedule drew no marker');
      const pct = parseFloat(mark.style.left);
      // The elapsed portion falls on deactivated days, so the scheduled marker
      // is frozen at 0% where the calendar marker would sit at ~14%.
      if (pct !== 0) throw new Error(`deactivated-elapsed schedule marker sat at ${mark.style.left}, expected 0%`);
    },
  },
  // Press-and-hold peek (shared card, so desktop and Android at once): holding
  // the bar swaps the frozen scheduled marker (0%) for the calendar marker
  // (~14%), and releasing reverts. Both surfaces implement the same transient,
  // unpersisted state.
  {
    file: 'src/lib/shared/UsageCard.svelte',
    props: () => ({ snap: { provider_name: 'Claude', fetched_at: new Date().toISOString(), error: null, credits: null, windows: [weeklyWindowOneDayIn()] }, schedule: elapsedDaysOffSchedule() }),
    verify: async ({ target, flushSync }) => {
      const bar = target.querySelector('.bar');
      const mark = bar.querySelector('.period-mark');
      if (!mark) throw new Error('weekly window with a schedule drew no marker');
      if (parseFloat(mark.style.left) !== 0) throw new Error(`scheduled marker sat at ${mark.style.left}, expected 0%`);
      bar.dispatchEvent(new window.PointerEvent('pointerdown', { bubbles: true }));
      flushSync();
      const peek = parseFloat(mark.style.left);
      if (!(peek > 13 && peek < 16)) throw new Error(`held marker sat at ${mark.style.left}, expected ~14%`);
      bar.dispatchEvent(new window.PointerEvent('pointerup', { bubbles: true }));
      flushSync();
      if (parseFloat(mark.style.left) !== 0) throw new Error(`released marker did not revert: ${mark.style.left}`);
    },
  },
  // The same peek on the mini summary: a weekly headline one day in, with the
  // elapsed days off, so the scheduled marker freezes at 0% and the calendar
  // marker sits at ~14%. Holding the row's bar swaps, releasing reverts. The
  // mini summary looks the schedule up from config by provider id, so the case
  // supplies both a custom snapshot (a weekly window with a bounded period) and
  // a config whose claude entry carries the deactivating schedule.
  {
    file: 'src/lib/MiniSummary.svelte',
    props: () => ({}),
    snapshots: [{ provider_id: 'claude', provider_name: 'Claude', error: null, credits: null, windows: [weeklyWindowOneDayIn()] }],
    config: { ...CONFIG, providers: { claude: provider({ enabled: true, mini_summary_metrics: ['window:weekly'], usage_schedule: elapsedDaysOffSchedule() }) } },
    verify: async ({ target, flushSync }) => {
      const mark = target.querySelector('.hover-bar-cell .period-mark');
      if (!mark) throw new Error('scheduled weekly row drew no marker');
      if (parseFloat(mark.style.left) !== 0) throw new Error(`scheduled marker sat at ${mark.style.left}, expected 0%`);
      const targetEl = target.querySelector('.hover-bar-target');
      targetEl.dispatchEvent(new window.PointerEvent('pointerdown', { bubbles: true }));
      flushSync();
      const peek = parseFloat(mark.style.left);
      if (!(peek > 13 && peek < 16)) throw new Error(`held marker sat at ${mark.style.left}, expected ~14%`);
      targetEl.dispatchEvent(new window.PointerEvent('pointerup', { bubbles: true }));
      flushSync();
      if (parseFloat(mark.style.left) !== 0) throw new Error(`released marker did not revert: ${mark.style.left}`);
    },
  },
  // Spend with no budget configured: the label must prefix the amount, so the
  // figure cannot be misread as money remaining. The unlabelled balance above
  // it must stay bare.
  {
    file: 'src/lib/shared/UsageCard.svelte',
    props: () => ({ snap: structuredClone(SNAPSHOTS.at(-1)) }),
    expect: ['Cost this month: 8.75 USD'],
  },
  // Z.ai's card (issue #183): percentage-only token windows render as plain
  // bars with no invented totals, and the monthly web/tool window renders
  // with its absolute counts but as an informational row.
  {
    file: 'src/lib/shared/UsageCard.svelte',
    props: () => ({ snap: structuredClone(SNAPSHOTS.find((s) => s.provider_id === 'zai')) }),
    expect: ['Z.ai', '5-hour', 'Weekly', 'Web/tool this month'],
    verify: ({ target }) => {
      const rows = [...target.querySelectorAll('.window')];
      if (rows.length !== 3) throw new Error(`expected 3 windows, got ${rows.length}`);
      const text = rows.map((row) => row.textContent);
      if (text.some((t) => /\d+\s*\/\s*\d+\s*tokens/i.test(t))) {
        throw new Error(`a token window invented absolute totals: ${text.join(' | ')}`);
      }
      const calls = text.find((t) => t.includes('calls'));
      if (!calls || !calls.includes('75') || !calls.includes('remaining')) {
        throw new Error(`the web/tool row did not carry its counts: ${text.join(' | ')}`);
      }
      // The informational row gets the muted fill, never a threshold colour.
      const muted = rows[2].querySelector('.fill.muted');
      if (!muted) throw new Error('the web/tool window was not rendered as informational');
    },
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
      account: { label: null, enabled: true, low_balance_warn: null, mini_summary_metrics: null, tray_metric: null, usage_schedule: { monday: true, tuesday: true, wednesday: true, thursday: true, friday: true, saturday: true, sunday: true }, settings: {} },
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
      // The usage-schedule editor rides alongside the headline picker: seven
      // weekday toggles, all on by default, and clicking one must uncheck it.
      const sched = [...target.querySelectorAll('.schedule-day input')];
      if (sched.length !== 7) throw new Error(`schedule editor had ${sched.length} toggles, expected 7`);
      if (!sched.every((t) => t.checked)) throw new Error('the default schedule did not render all seven on');
      sched[5].click();
      flushSync();
      if (sched[5].checked) throw new Error('clicking a schedule toggle did not uncheck it');
      [...target.querySelectorAll('.provider-footer button')].find((b) => b.textContent.trim() === 'Test').click();
      if (!globalThis.__SMOKE_TESTED__) throw new Error('Test did not reach onTest');
      [...target.querySelectorAll('.provider-footer button')].find((b) => b.textContent.trim() === 'Remove account').click();
      if (!globalThis.__SMOKE_REMOVED__) throw new Error('Remove did not reach onRemove');
    },
  },
  // OAuthAccount: the Android built-in sign-in row for Claude/Codex.
  // Renders the signed-out, pending and signed-in states without throwing.
  {
    file: 'src/mobile/OAuthAccount.svelte',
    props: () => ({
      id: 'claude',
      kind: 'claude',
      account: { label: null, enabled: true, low_balance_warn: null, mini_summary_metrics: null, tray_metric: null, usage_schedule: { monday: true, tuesday: true, wednesday: true, thursday: true, friday: true, saturday: true, sunday: true }, settings: {} },
      providerName: 'Claude',
      providerNote: 'Built-in browser sign-in.',
      pending: null,
      secretStored: false,
      headlineOptions: [{ id: 'window:five_hour', label: '5-hour' }],
      headlineOpen: false,
      onToggleHeadline() {},
      expanded: true,
      onSignIn() { globalThis.__SMOKE_OAUTH_SIGNIN__ = true; },
      onCompleteClaude() { globalThis.__SMOKE_OAUTH_COMPLETE__ = true; },
      onPollCodex() {},
      onCancel() { globalThis.__SMOKE_OAUTH_CANCEL__ = true; },
      onRemove() { globalThis.__SMOKE_OAUTH_REMOVE__ = true; },
    }),
    expect: ['Sign in with Claude', 'Remove account', 'Usage days'],
    verify: async ({ target, flushSync }) => {
      const signIn = [...target.querySelectorAll('button')]
        .find((b) => b.textContent.trim() === 'Sign in with Claude');
      if (!signIn) throw new Error('Sign in button was not rendered');
      // The schedule editor renders on the OAuth row too (it is shown on every
      // account), seven toggles all on by default.
      const sched = [...target.querySelectorAll('.schedule-day input')];
      if (sched.length !== 7) throw new Error(`schedule editor had ${sched.length} toggles, expected 7`);
      if (!sched.every((t) => t.checked)) throw new Error('the default schedule did not render all seven on');
      signIn.click();
      if (!globalThis.__SMOKE_OAUTH_SIGNIN__) throw new Error('Sign in did not reach onSignIn');
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
      openSection(target, 'providers', flushSync);
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
  // Mobile usage-schedule editor (issue #123), end to end through the shell: a
  // config that predates the field (no usage_schedule at all) must load with
  // all seven days on — MobileApp.syncAccountState backfills it the way
  // quota-core's serde default does — and editing a day then Saving must
  // persist through set_config. This is the mobile mirror of the desktop
  // Settings schedule case above.
  {
    file: 'src/mobile/MobileApp.svelte',
    props: () => ({}),
    config: (() => {
      // Simulate an upgrade: openrouter's schedule field is entirely absent.
      const { usage_schedule, ...openrouter } = provider({});
      return { ...CONFIG, providers: { ...CONFIG.providers, openrouter } };
    })(),
    verify: async ({ target, flushSync }) => {
      target.querySelector('button[title="Settings"]').click();
      flushSync();
      openSection(target, 'providers', flushSync);
      const card = [...target.querySelectorAll('.provider')]
        .find((el) => el.querySelector('strong').textContent.trim() === 'OpenRouter');
      if (!card) throw new Error('the OpenRouter account row did not render in mobile settings');
      card.querySelector('.provider-disclosure').click();
      flushSync();
      const sched = [...card.querySelectorAll('.schedule-day input')];
      if (sched.length !== 7) throw new Error(`mobile schedule editor had ${sched.length} toggles, expected 7`);
      // An absent field loads as all-seven, not as seven unset checkboxes.
      if (!sched.every((t) => t.checked)) {
        throw new Error('an absent usage_schedule did not default to all seven on in mobile settings');
      }
      sched[6].click(); // turn Sunday off
      flushSync();
      if (sched[6].checked) throw new Error('clicking a mobile schedule toggle did not uncheck it');
      [...target.querySelectorAll('button')].find((b) => b.textContent.trim() === 'Save').click();
      await new Promise((resolve) => setTimeout(resolve, 0));
      const saved = globalThis.__SMOKE_LAST_CONFIG__?.providers?.openrouter?.usage_schedule;
      const weekdays = ['monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday'];
      if (!saved || weekdays.some((k) => saved[k] !== true) || saved.sunday !== false) {
        throw new Error(`saved mobile schedule was ${JSON.stringify(saved)}`);
      }
    },
  },
  // MobileApp OAuth sign-in through the *parent* callback path (issue #134).
  // The child-component OAuthAccount case above mounts OAuthAccount directly
  // with its own working `onSignIn`, so it never exercises MobileApp's own
  // dispatch arrow — which referenced a `kind` that is out of scope inside the
  // provider `{#each}` and threw `kind is not defined` the instant Sign in was
  // pressed. This mounts the whole shell with a Claude and a Codex OAuth
  // account and drives both flows end to end: Sign in must reach the *right*
  // start command (proving the selected provider was dispatched, not a crash),
  // the pending state that comes back must switch the row into its
  // paste/poll UI, and completing must reach finish/poll for that provider.
  {
    file: 'src/mobile/MobileApp.svelte',
    props: () => ({}),
    config: { ...CONFIG, providers: { claude: provider({ enabled: true }), codex: provider({ enabled: true }) } },
    verify: async ({ target, flushSync, signinCalls }) => {
      // startClaude opens the authorize URL; jsdom's window.open is a noisy
      // no-op, so stub it out rather than let it warn mid-run.
      window.open = () => null;
      const card = (name) => [...target.querySelectorAll('.provider')]
        .find((el) => el.querySelector('strong')?.textContent.trim() === name);
      const button = (root, text) => [...root.querySelectorAll('button')]
        .find((b) => b.textContent.trim() === text);

      target.querySelector('button[title="Settings"]').click();
      flushSync();
      openSection(target, 'providers', flushSync);

      // Claude: expand, Sign in → the parent must dispatch start_claude_signin
      // for *this* provider. Before the fix this line threw before any command
      // was sent, so the assertion is both "no crash" and "right provider".
      const claude = card('Claude');
      if (!claude) throw new Error('the Claude OAuth row did not render');
      claude.querySelector('.provider-disclosure').click();
      flushSync();
      button(claude, 'Sign in with Claude').click();
      await settle(flushSync);
      if (!signinCalls().includes('start_claude_signin:claude')) {
        throw new Error(`Sign in did not dispatch the Claude start command: ${signinCalls().join(',') || 'nothing'}`);
      }
      // The pending state that came back must switch the row to paste-back.
      const codeInput = card('Claude').querySelector('input[placeholder="code#state"]');
      if (!codeInput) throw new Error('a pending Claude sign-in did not reveal the code paste field');
      codeInput.value = 'the-code#the-state';
      codeInput.dispatchEvent(new window.Event('input', { bubbles: true }));
      flushSync();
      button(card('Claude'), 'Complete sign-in').click();
      await settle(flushSync);
      if (!signinCalls().includes('finish_claude_signin:claude#the-code#the-state')) {
        throw new Error(`Complete sign-in did not reach finish_claude_signin: ${signinCalls().join(',')}`);
      }

      // Codex: the other branch of the same dispatch arrow must reach the Codex
      // start command, and its pending state polls rather than pastes.
      const codex = card('Codex');
      if (!codex) throw new Error('the Codex OAuth row did not render');
      codex.querySelector('.provider-disclosure').click();
      flushSync();
      button(codex, 'Sign in with Codex').click();
      await settle(flushSync);
      if (!signinCalls().includes('start_codex_signin:codex')) {
        throw new Error(`Sign in did not dispatch the Codex start command: ${signinCalls().join(',') || 'nothing'}`);
      }
      if (!card('Codex').textContent.includes('SMOKE-CODE')) {
        throw new Error('a pending Codex sign-in did not show the device user code');
      }
      button(card('Codex'), 'I entered the code — check').click();
      await settle(flushSync);
      if (!signinCalls().includes('poll_codex_signin:codex')) {
        throw new Error(`the Codex check did not reach poll_codex_signin: ${signinCalls().join(',')}`);
      }
    },
  },
  // Credential export & import through the mobile shell (issue #152). The
  // system dialog and the two commands are stubbed; what is proven here is
  // the wiring the issue's criteria depend on: the typed passphrase reaches
  // the command together with the file the dialog returned, an import report
  // renders as the four-way summary (added / updated / needs sign-in /
  // could-not-store), and a refused import surfaces the error without any
  // set_config — existing accounts untouched.
  {
    file: 'src/mobile/MobileApp.svelte',
    props: () => ({}),
    config: { ...CONFIG, providers: { openrouter: provider({ enabled: true }) } },
    verify: async ({ target, flushSync }) => {
      const button = (text) => [...target.querySelectorAll('button')].find((b) => b.textContent.trim() === text);
      const fillPasswords = (values) => {
        const inputs = [...target.querySelectorAll('.credential-transfer input[type="password"]')];
        if (inputs.length !== values.length) throw new Error(`expected ${values.length} passphrase fields, found ${inputs.length}`);
        inputs.forEach((el, i) => {
          el.value = values[i];
          el.dispatchEvent(new window.Event('input', { bubbles: true }));
        });
        flushSync();
      };
      target.querySelector('button[title="Settings"]').click();
      flushSync();
      openSection(target, 'transfer', flushSync);

      // Export: both passphrase fields, then the picked file and the
      // passphrase reach export_credentials together.
      button('Export accounts…').click();
      flushSync();
      globalThis.__SMOKE_DIALOG_SAVE__ = 'content://smoke/export.qwb';
      fillPasswords(['correct horse', 'correct horse']);
      button('Choose where to save').click();
      await settle(flushSync);
      const calls = globalThis.__SMOKE_TRANSFER_CALLS__;
      if (!calls.includes('export_credentials:content://smoke/export.qwb:correct horse')) {
        throw new Error(`export did not reach the command with the picked file and passphrase: ${calls.join(',') || 'nothing'}`);
      }
      if (!target.textContent.includes('Export written')) {
        throw new Error('a successful export showed no confirmation');
      }

      // Import: the report renders as the four-way summary, and the shell
      // reloads (handleImported re-reads the config for the new accounts).
      button('Import accounts…').click();
      flushSync();
      globalThis.__SMOKE_DIALOG_OPEN__ = 'content://smoke/export.qwb';
      globalThis.__SMOKE_IMPORT_REPORT__ = {
        accounts: {
          elevenlabs: { outcome: 'added' },
          'openrouter#1': { outcome: 'updated' },
          claude: { outcome: 'needs_onboarding' },
          deepseek: { outcome: 'could_not_store', reason: 'keystore locked' },
        },
      };
      fillPasswords(['correct horse']);
      button('Choose the export file').click();
      await settle(flushSync);
      if (!calls.includes('import_credentials:content://smoke/export.qwb:correct horse')) {
        throw new Error(`import did not reach the command: ${calls.join(',')}`);
      }
      const summary = target.querySelector('.import-summary');
      for (const text of ['Added: elevenlabs', 'Updated: openrouter#1', 'Needs sign-in: claude', 'Could not store deepseek: keystore locked']) {
        if (!summary?.textContent.includes(text)) {
          throw new Error(`summary missing ${JSON.stringify(text)}: ${summary?.textContent}`);
        }
      }

      // A refused import (wrong passphrase / corrupt file) surfaces the error
      // and saves nothing — the accounts on screen are the ones that were
      // there before.
      const savesBefore = globalThis.__SMOKE_LAST_CONFIG__;
      button('Import accounts…').click();
      flushSync();
      globalThis.__SMOKE_IMPORT_ERROR__ = 'wrong passphrase, or the sealed bundle was tampered with or truncated';
      fillPasswords(['wrong']);
      button('Choose the export file').click();
      await settle(flushSync);
      if (!target.querySelector('.credential-transfer .test.bad')?.textContent.includes('wrong passphrase')) {
        throw new Error(`a refused import did not surface the error: ${target.querySelector('.credential-transfer')?.textContent}`);
      }
      if (globalThis.__SMOKE_LAST_CONFIG__ !== savesBefore) {
        throw new Error('a refused import still wrote a config');
      }
    },
  },
  // Desktop→phone QR transfer, desktop side (issue #156): typing and
  // confirming a passphrase reaches qr_transfer_frames, and the returned SVG
  // renders — proving the frame actually reaches the page, not just the
  // command call. A mismatched confirmation is refused before the command.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    qrFrames: ['<svg data-smoke-frame="0"><rect width="1" height="1"/></svg>'],
    verify: async ({ target, flushSync }) => {
      openSection(target, 'transfer', flushSync);
      const button = (text) => [...target.querySelectorAll('button')].find((b) => b.textContent.trim() === text);
      const fillPasswords = (values) => {
        const inputs = [...target.querySelectorAll('.qr-transfer input[type="password"]')];
        if (inputs.length !== values.length) throw new Error(`expected ${values.length} passphrase fields, found ${inputs.length}`);
        inputs.forEach((el, i) => {
          el.value = values[i];
          el.dispatchEvent(new window.Event('input', { bubbles: true }));
        });
        flushSync();
      };

      button('Show QR code…').click();
      flushSync();
      fillPasswords(['one', 'two']);
      button('Show code').click();
      await settle(flushSync);
      if (!globalThis.__SMOKE_TRANSFER_CALLS__.every((c) => !c.startsWith('qr_transfer_frames'))) {
        throw new Error('a mismatched confirmation still called qr_transfer_frames');
      }
      if (!target.querySelector('.qr-transfer .test.bad')) {
        throw new Error('a mismatched confirmation showed no error');
      }

      fillPasswords(['correct horse', 'correct horse']);
      button('Show code').click();
      await settle(flushSync);
      if (!globalThis.__SMOKE_TRANSFER_CALLS__.includes('qr_transfer_frames:correct horse')) {
        throw new Error(`did not reach qr_transfer_frames with the passphrase: ${globalThis.__SMOKE_TRANSFER_CALLS__.join(',')}`);
      }
      if (!target.querySelector('.qr-frame svg')) {
        throw new Error('the returned frame did not render as an SVG on the page');
      }

      button('Done').click();
      flushSync();
      if (target.querySelector('.qr-frame') || !button('Show QR code…')) {
        throw new Error('Done did not return to the closed state');
      }
    },
  },
  // LAN desktop pairing (issue #154), sender and receiver flows. Proven here:
  // an incomplete code never reaches the command; the typed code and address
  // reach lan_pairing_send; Generate's code lands in the field and reaches
  // lan_pairing_receive_start; the single lan-pairing event renders the
  // four-way summary, clears the waiting state and burns the code; a cancel
  // reaches the shell. The transfer itself is proven at the
  // quota_core::pairing seam in Rust.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    verify: async ({ target, flushSync, emit }) => {
      openSection(target, 'pairing', flushSync);
      const button = (text) => [...target.querySelectorAll('button')].find((b) => b.textContent.trim() === text);
      const fill = (values) => {
        const inputs = [...target.querySelectorAll('.lan-pairing input')];
        if (inputs.length !== values.length) throw new Error(`expected ${values.length} pairing inputs, found ${inputs.length}`);
        inputs.forEach((el, i) => {
          el.value = values[i];
          el.dispatchEvent(new window.Event('input', { bubbles: true }));
        });
        flushSync();
      };
      const calls = () => globalThis.__SMOKE_TRANSFER_CALLS__;

      // Sender: an incomplete code is refused before any command runs.
      button('Send to another device…').click();
      flushSync();
      fill(['12345', '192.168.1.20']);
      button('Send accounts').click();
      await settle(flushSync);
      if (calls().some((c) => c.startsWith('lan_pairing_send'))) {
        throw new Error(`an incomplete code still called the send command: ${calls().join(',')}`);
      }
      const notes = [...target.querySelectorAll('.lan-pairing .note')].map((p) => p.textContent).join(' ');
      if (!notes.includes('6-digit')) {
        throw new Error('an incomplete code showed no explanation');
      }

      // Sender: the typed code and address reach the command, and the
      // success message promises the summary appears on the other device.
      fill(['654321', ' 192.168.1.20 ']);
      button('Send accounts').click();
      await settle(flushSync);
      if (!calls().includes('lan_pairing_send:654321:192.168.1.20')) {
        throw new Error(`did not reach lan_pairing_send with code and address: ${calls().join(',')}`);
      }
      const sentNotes = [...target.querySelectorAll('.lan-pairing .note')].map((p) => p.textContent).join(' ');
      if (!sentNotes.includes('Accounts sent')) {
        throw new Error('a successful send showed no confirmation');
      }

      // Sender: a send held open by a stalled receiver is cancelable —
      // Cancel reaches the shell, the pending send rejects, and the panel
      // comes back ready to use instead of spinning forever.
      globalThis.__SMOKE_LAN_SEND_HANG__ = true;
      fill(['654321', '192.168.1.20']);
      button('Send accounts').click();
      flushSync();
      const sending = [...target.querySelectorAll('.lan-pairing button')].find((b) => b.textContent.trim() === 'Sending…');
      if (!sending || sending.disabled !== true) {
        throw new Error('an in-flight send showed no disabled Sending… button');
      }
      const sendCancel = [...target.querySelectorAll('.lan-pairing button')].find((b) => b.textContent.trim() === 'Cancel');
      if (sendCancel.disabled) {
        throw new Error('Cancel is disabled while sending — the stall would be unescapable');
      }
      sendCancel.click();
      await settle(flushSync);
      if (!calls().includes('lan_pairing_cancel')) {
        throw new Error(`Cancel during a send did not reach lan_pairing_cancel: ${calls().join(',')}`);
      }
      if (target.querySelector('.lan-pairing input')) {
        throw new Error('cancelling a send left the send form open');
      }
      globalThis.__SMOKE_LAN_SEND_HANG__ = false;

      // Receiver: Generate fills the code, arming consumes exactly it.
      button('Receive on this device…').click();
      await settle(flushSync); // the address lookup resolves after the click
      if (!target.querySelector('.lan-pairing .device-code')?.textContent.includes('192.168.1.20:45454')) {
        throw new Error(`the receiver did not show the device address: ${target.querySelector('.lan-pairing')?.textContent}`);
      }
      button('Generate').click();
      await settle(flushSync);
      const codeInput = target.querySelector('.lan-pairing input');
      if (codeInput.value !== '428194') {
        throw new Error(`Generate did not fill the code field: ${codeInput.value}`);
      }
      button('Wait for the sender').click();
      await settle(flushSync);
      if (!calls().includes('lan_pairing_receive_start:428194')) {
        throw new Error(`arming did not reach receive_start with the shown code: ${calls().join(',')}`);
      }
      if (!target.querySelector('.lan-pairing .note')?.textContent.includes('Waiting for the other device')) {
        throw new Error('an armed session showed no waiting state');
      }

      // The armed session's one outcome: the four-way summary, the waiting
      // state cleared, and the code burned.
      await emit('lan-pairing', {
        ok: true,
        report: {
          accounts: {
            elevenlabs: { outcome: 'added' },
            'openrouter#1': { outcome: 'updated' },
            claude: { outcome: 'needs_onboarding' },
          },
        },
      });
      const summary = [...target.querySelectorAll('.lan-pairing .note')].map((p) => p.textContent).join(' ');
      for (const text of ['Added 1, updated 1, 1 awaiting sign-in', 'Waiting for the other device']) {
        const present = summary.includes(text);
        const wanted = text !== 'Waiting for the other device';
        if (present !== wanted) {
          throw new Error(`after the result, ${wanted ? 'missing' : 'stale'}: ${text}`);
        }
      }
      if (target.querySelector('.lan-pairing input').value !== '') {
        throw new Error('the armed code survived its session — it must be single-use');
      }

      // A cancelled session reaches the shell and leaves the waiting state.
      button('Generate').click();
      await settle(flushSync);
      button('Wait for the sender').click();
      await settle(flushSync);
      button('Cancel').click();
      await settle(flushSync);
      if (!calls().includes('lan_pairing_cancel')) {
        throw new Error('Cancel did not reach lan_pairing_cancel');
      }
      if (target.querySelector('.lan-pairing .note')?.textContent.includes('Waiting for the other device')) {
        throw new Error('a cancelled session is still shown as waiting');
      }
    },
  },
  // Desktop→phone QR transfer, mobile side (issue #156): the scan loop calls
  // qr_scan_reset then qr_scan_frame for each queued detection, renders
  // "captured N of M" progress from the FrameStatus each call returns, and —
  // once the collector reports complete — asks for the passphrase and
  // reaches qr_scan_finish with it, rendering the same four-way summary the
  // file import path does.
  {
    file: 'src/mobile/MobileApp.svelte',
    props: () => ({}),
    config: { ...CONFIG, providers: { openrouter: provider({ enabled: true }) } },
    verify: async ({ target, flushSync }) => {
      const button = (text) => [...target.querySelectorAll('button')].find((b) => b.textContent.trim() === text);
      target.querySelector('button[title="Settings"]').click();
      flushSync();
      openSection(target, 'transfer', flushSync);

      globalThis.__SMOKE_QR_SCAN_QUEUE__ = ['frame-a', 'frame-b'];
      globalThis.__SMOKE_QR_STATUS_SEQUENCE__ = [
        { have: 1, total: 2, complete: false },
        { have: 2, total: 2, complete: true },
      ];
      button('Scan from desktop…').click();
      await settle(flushSync);
      const calls = globalThis.__SMOKE_TRANSFER_CALLS__;
      if (calls[0] !== 'qr_scan_reset') {
        throw new Error(`scan did not reset the collector first: ${calls.join(',')}`);
      }
      if (!calls.includes('qr_scan_frame:frame-a') || !calls.includes('qr_scan_frame:frame-b')) {
        throw new Error(`both scanned frames did not reach qr_scan_frame: ${calls.join(',')}`);
      }
      // Progress ("captured N of M") only renders while still in the
      // qr-scanning view; completion moves straight to the passphrase
      // prompt, which is what's asserted below.
      const passInput = target.querySelector('.credential-transfer input[type="password"]');
      if (!passInput) throw new Error('scan completing did not prompt for the passphrase');
      passInput.value = 'correct horse';
      passInput.dispatchEvent(new window.Event('input', { bubbles: true }));
      flushSync();

      globalThis.__SMOKE_IMPORT_REPORT__ = {
        accounts: {
          elevenlabs: { outcome: 'added' },
          claude: { outcome: 'needs_onboarding' },
        },
      };
      button('Import').click();
      await settle(flushSync);
      if (!calls.includes('qr_scan_finish:correct horse')) {
        throw new Error(`did not reach qr_scan_finish with the passphrase: ${calls.join(',')}`);
      }
      const summary = target.querySelector('.import-summary');
      if (!summary?.textContent.includes('Added: elevenlabs') || !summary.textContent.includes('Needs sign-in: claude')) {
        throw new Error(`scan summary missing expected outcomes: ${summary?.textContent}`);
      }
    },
  },
  // LAN pairing on the phone (issue #155), end to end through the shell: the
  // receive flow arms the generated code (the same commands desktop's
  // Settings drives — the session rules live once in Rust `lan_pairing.rs`),
  // the `lan-pairing` event lands the four-way named-list summary this
  // component shares with the file/QR imports, a failed attempt leaves an
  // explanatory message, and Cancel reaches the shell. The address fallback,
  // code burning and single-use semantics mirror the desktop case above.
  {
    file: 'src/mobile/MobileApp.svelte',
    props: () => ({}),
    config: { ...CONFIG, providers: { openrouter: provider({ enabled: true }) } },
    verify: async ({ target, flushSync, emit }) => {
      const button = (text) => [...target.querySelectorAll('button')].find((b) => b.textContent.trim() === text);
      target.querySelector('button[title="Settings"]').click();
      flushSync();
      openSection(target, 'pairing', flushSync);

      const section = target.querySelector('.lan-pairing');
      if (!section) throw new Error('the mobile settings did not offer LAN pairing');
      const pair = (text) => [...section.querySelectorAll('button')].find((b) => b.textContent.trim() === text);
      const codeInput = () => section.querySelector('input[inputmode="numeric"]');

      // Receive: this phone shows its address, arms the generated code.
      globalThis.__SMOKE_LAN_ADDRESS__ = ['192.168.1.20'];
      pair('Receive on this phone…').click();
      await settle(flushSync);
      if (!section.textContent.includes('192.168.1.20')) {
        throw new Error(`the receive flow did not show the device address: ${section.textContent}`);
      }
      pair('Generate').click();
      await settle(flushSync);
      if (codeInput()?.value !== '428194') {
        throw new Error(`Generate did not fill the code input: ${codeInput()?.value}`);
      }
      pair('Wait for the sender').click();
      await settle(flushSync);
      const calls = globalThis.__SMOKE_TRANSFER_CALLS__;
      if (!calls.includes('lan_pairing_receive_start:428194')) {
        throw new Error(`arming did not reach lan_pairing_receive_start: ${calls.join(',')}`);
      }
      if (!section.textContent.includes('Waiting for the other device')) {
        throw new Error('an armed session did not say it was waiting');
      }

      // The armed session's one outcome: the named-list summary the file/QR
      // imports render, the waiting state cleared, and the code burned.
      await emit('lan-pairing', {
        ok: true,
        report: {
          accounts: {
            elevenlabs: { outcome: 'added' },
            'openrouter#1': { outcome: 'updated' },
            claude: { outcome: 'needs_onboarding' },
            deepseek: { outcome: 'could_not_store', reason: 'keystore locked' },
          },
        },
      });
      await settle(flushSync);
      const summary = [...section.querySelectorAll('.import-summary p')].map((p) => p.textContent).join(' ');
      for (const text of [
        'Added: elevenlabs',
        'Updated: openrouter#1',
        'Needs sign-in: claude',
        'Could not store deepseek: keystore locked. The account was not added.',
      ]) {
        if (!summary.includes(text)) throw new Error(`the receive summary is missing "${text}": ${summary}`);
      }
      if (codeInput()?.value !== '') {
        throw new Error('the armed code survived its session — it must be single-use');
      }

      // A failed attempt (wrong code upstream, stall, whatever) reads as a
      // message, not a summary, and still spends the code.
      pair('Generate').click();
      await settle(flushSync);
      pair('Wait for the sender').click();
      await settle(flushSync);
      await emit('lan-pairing', { ok: false, error: 'The transfer stalled — nothing was changed.' });
      await settle(flushSync);
      if (section.querySelector('.import-summary')) {
        throw new Error('a stale summary survived into a failed attempt');
      }
      if (!section.textContent.includes('The transfer stalled — nothing was changed.')) {
        throw new Error(`the failure was not explained: ${section.textContent}`);
      }

      // A cancelled session reaches the shell.
      pair('Generate').click();
      await settle(flushSync);
      pair('Wait for the sender').click();
      await settle(flushSync);
      pair('Cancel').click();
      await settle(flushSync);
      if (!calls.includes('lan_pairing_cancel')) {
        throw new Error('Cancel did not reach lan_pairing_cancel');
      }
    },
  },
  // ---- Issue #184: top-level Settings sections are collapsible disclosures ----
  // The desktop form, end to end: every section starts collapsed on arrival,
  // each heading is a semantic button wired to its own panel by aria-controls /
  // aria-expanded, several sections can be open at once (disclosure, not
  // accordion), collapsing one leaves the others untouched, and an unsaved
  // edit survives the panel being destroyed and rebuilt across a collapse.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    desktop: { state: 'absent', appimage: '/home/u/Downloads/q.AppImage', desktop_file: '/home/u/.local/share/applications/quota-widget.desktop' },
    verify: async ({ target, flushSync }) => {
      const ids = ['providers', 'thresholds', 'alerts', 'general', 'backup', 'transfer', 'pairing', 'desktop'];
      // Nothing is open on arrival: every heading is a real button that points
      // at a panel not yet in the DOM.
      for (const id of ids) {
        const btn = target.querySelector(`.disclosure > h2 > button#${id}-toggle`);
        if (!btn) throw new Error(`section "${id}" rendered no heading button`);
        if (btn.tagName !== 'BUTTON' || btn.getAttribute('type') !== 'button') {
          throw new Error(`section "${id}" heading is not a semantic button`);
        }
        if (btn.getAttribute('aria-controls') !== `${id}-panel`) {
          throw new Error(`section "${id}" button does not point at its panel`);
        }
        if (btn.getAttribute('aria-expanded') !== 'false') {
          throw new Error(`section "${id}" did not start collapsed`);
        }
        if (target.querySelector(`#${id}-panel`)) {
          throw new Error(`section "${id}" rendered its panel while collapsed`);
        }
      }
      // Opening a section reveals a region labelled by its own heading.
      openSection(target, 'thresholds', flushSync);
      const panel = target.querySelector('#thresholds-panel');
      if (panel.getAttribute('role') !== 'region'
        || panel.getAttribute('aria-labelledby') !== 'thresholds-toggle') {
        throw new Error('an opened panel is not a region labelled by its heading');
      }
      // A second section opens alongside the first — disclosure, not accordion.
      openSection(target, 'general', flushSync);
      if (!target.querySelector('#thresholds-panel') || !target.querySelector('#general-panel')) {
        throw new Error('opening a second section collapsed the first');
      }
      // An unsaved edit is bound to the form's state, not the panel DOM, so it
      // outlives the panel being destroyed on collapse and rebuilt on reopen.
      const warn = () => target.querySelector('#thresholds-panel input.num');
      warn().value = '37';
      warn().dispatchEvent(new window.Event('input', { bubbles: true }));
      flushSync();
      target.querySelector('button#thresholds-toggle').click(); // collapse
      flushSync();
      if (target.querySelector('#thresholds-panel')) throw new Error('collapsing left the panel in the DOM');
      if (target.querySelector('button#thresholds-toggle').getAttribute('aria-expanded') !== 'false') {
        throw new Error('a collapsed section still reported aria-expanded=true');
      }
      if (!target.querySelector('#general-panel')) throw new Error('collapsing thresholds also closed general');
      openSection(target, 'thresholds', flushSync); // reopen
      if (warn().value !== '37') {
        throw new Error(`the unsaved edit did not survive a collapse/reopen: ${warn().value}`);
      }
    },
  },
  // Issue #184: the per-account disclosures inside Providers own their own open
  // state, independent of the section wrapper — toggling a section, or even
  // collapsing and reopening Providers itself, does not reset an expanded
  // account — and a flow whose result lands in a closed section (an inbound
  // LAN transfer) opens that section from the outside.
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    verify: async ({ target, flushSync, emit }) => {
      openSection(target, 'providers', flushSync);
      const claude = () => [...target.querySelectorAll('.provider')]
        .find((el) => el.querySelector('strong').textContent.trim() === 'Claude');
      claude().querySelector('.provider-disclosure').click(); // expand the account
      flushSync();
      if (!claude().querySelector('.metric-picker')) throw new Error('expanding the account revealed no editor');
      // Toggling an unrelated top-level section leaves the account editor open.
      openSection(target, 'thresholds', flushSync);
      if (!claude().querySelector('.metric-picker')) throw new Error('opening another section collapsed the account');
      // Collapsing and reopening Providers preserves the account's own state:
      // the two disclosures do not share a flag.
      target.querySelector('button#providers-toggle').click();
      flushSync();
      openSection(target, 'providers', flushSync);
      if (!claude().querySelector('.metric-picker')) {
        throw new Error('the account disclosure reset when its section reopened');
      }
      // A closed section can be opened by a flow whose result belongs in it:
      // an inbound LAN pairing result reveals the Pairing section.
      if (target.querySelector('#pairing-panel')) throw new Error('pairing started open');
      await emit('lan-pairing', { ok: false, error: 'nothing changed' });
      if (!target.querySelector('#pairing-panel')) {
        throw new Error('an inbound LAN result did not reveal the Pairing section');
      }
    },
  },
  // Issue #184 on Android: the whole settings screen, section by section.
  // Every one of the seven top-level sections starts collapsed, carries the
  // same semantic-button + aria-controls/aria-expanded wiring as desktop,
  // opens to reveal a labelled region, and stays open when its neighbours are
  // opened too (disclosure, not accordion). Asserted per id so a regression in
  // any single section is caught, not just Providers.
  {
    file: 'src/mobile/MobileApp.svelte',
    props: () => ({}),
    config: { ...CONFIG, providers: { openrouter: provider({ enabled: true }) } },
    verify: async ({ target, flushSync }) => {
      const ids = ['providers', 'transfer', 'pairing', 'ordering', 'thresholds', 'notifications', 'background'];
      target.querySelector('button[title="Settings"]').click();
      flushSync();
      if (!target.querySelector('.mobile-settings')) throw new Error('the gear did not open mobile Settings');
      // Each section arrives collapsed, with a real button pointing at a panel
      // that is not yet in the DOM.
      for (const id of ids) {
        const btn = target.querySelector(`.disclosure > h2 > button#${id}-toggle`);
        if (!btn) throw new Error(`mobile section "${id}" rendered no heading button`);
        if (btn.tagName !== 'BUTTON' || btn.getAttribute('type') !== 'button') {
          throw new Error(`mobile section "${id}" heading is not a semantic button`);
        }
        if (btn.getAttribute('aria-controls') !== `${id}-panel`) {
          throw new Error(`mobile section "${id}" button does not point at its panel`);
        }
        if (btn.getAttribute('aria-expanded') !== 'false') {
          throw new Error(`mobile section "${id}" did not start collapsed`);
        }
        if (target.querySelector(`#${id}-panel`)) {
          throw new Error(`mobile section "${id}" rendered its panel while collapsed`);
        }
      }
      // Open every section in turn. openSection re-checks the aria wiring and
      // that the panel appears; each open must reveal a region labelled by its
      // own heading.
      for (const id of ids) {
        openSection(target, id, flushSync);
        const panel = target.querySelector(`#${id}-panel`);
        if (panel.getAttribute('role') !== 'region'
          || panel.getAttribute('aria-labelledby') !== `${id}-toggle`) {
          throw new Error(`mobile section "${id}" panel is not a region labelled by its heading`);
        }
      }
      // Every section stays open at once — opening later ones did not collapse
      // the earlier ones.
      for (const id of ids) {
        if (target.querySelector(`.disclosure > h2 > button#${id}-toggle`).getAttribute('aria-expanded') !== 'true') {
          throw new Error(`mobile section "${id}" closed when another section was opened`);
        }
        if (!target.querySelector(`#${id}-panel`)) {
          throw new Error(`mobile section "${id}" lost its panel while others were opened`);
        }
      }
      // Independence the other way: collapsing one leaves the rest open.
      target.querySelector('.disclosure > h2 > button#ordering-toggle').click();
      flushSync();
      if (target.querySelector('#ordering-panel')) throw new Error('collapsing ordering left its panel in the DOM');
      for (const id of ids.filter((i) => i !== 'ordering')) {
        if (!target.querySelector(`#${id}-panel`)) {
          throw new Error(`collapsing ordering also closed "${id}"`);
        }
      }
    },
  },
  // Issue #184 on Android: each visit to Settings starts fully collapsed, even
  // after a previous visit left a section open. The desktop form gets this for
  // free by remounting per visit; the mobile shell keeps one instance, so
  // MobileApp resets the flags on entry — this proves that reset.
  {
    file: 'src/mobile/MobileApp.svelte',
    props: () => ({}),
    config: { ...CONFIG, providers: { openrouter: provider({ enabled: true }) } },
    verify: async ({ target, flushSync }) => {
      const gear = () => target.querySelector('button[title="Settings"]');
      const back = () => target.querySelector('.mobile-header button[title="Back"]');
      const providersBtn = () => target.querySelector('.disclosure > h2 > button#providers-toggle');
      gear().click();
      flushSync();
      if (providersBtn().getAttribute('aria-expanded') !== 'false') {
        throw new Error('mobile Settings did not open collapsed');
      }
      openSection(target, 'providers', flushSync); // leave a section open
      back().click();
      flushSync();
      if (target.querySelector('.mobile-settings')) throw new Error('Back did not return to the list');
      gear().click();
      flushSync();
      if (providersBtn().getAttribute('aria-expanded') !== 'false') {
        throw new Error('a section left open on the previous visit was not reset on return');
      }
      if (target.querySelector('#providers-panel')) {
        throw new Error('the reopened Settings still rendered a section panel');
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
    // Mobile-only sign-in commands (issue #110/#134). The smoke harness has no
    // real browser or OAuth server, so start/finish/poll stand in for the Rust
    // host — but they are stateful over the pending list and record which
    // command the *parent* dispatched, so a case can prove MobileApp's callback
    // selected the right provider (the "kind is not defined" defect, #134) and
    // drove the pending -> signed-in transition. A real Android build exercises
    // the actual OAuth flows.
    case 'get_pending_signins': return globalThis.__SMOKE_PENDING_SIGNINS__ ?? [];
    case 'start_claude_signin':
      (globalThis.__SMOKE_SIGNIN_CALLS__ ??= []).push(\`start_claude_signin:\${args.provider}\`);
      (globalThis.__SMOKE_PENDING_SIGNINS__ ??= []).push({
        provider_key: args.provider, kind: 'claude_pkce',
        user_code: null, verification_url: null,
        expires_at: new Date(Date.now() + 15 * 60_000).toISOString(),
      });
      return 'https://claude.ai/oauth/authorize?smoke=1';
    case 'finish_claude_signin':
      (globalThis.__SMOKE_SIGNIN_CALLS__ ??= []).push(\`finish_claude_signin:\${args.provider}#\${args.code}\`);
      globalThis.__SMOKE_PENDING_SIGNINS__ = (globalThis.__SMOKE_PENDING_SIGNINS__ ?? []).filter((p) => p.provider_key !== args.provider);
      return null;
    case 'start_codex_signin':
      (globalThis.__SMOKE_SIGNIN_CALLS__ ??= []).push(\`start_codex_signin:\${args.provider}\`);
      (globalThis.__SMOKE_PENDING_SIGNINS__ ??= []).push({
        provider_key: args.provider, kind: 'codex_device',
        user_code: 'SMOKE-CODE', verification_url: 'https://auth.openai.com/codex/device',
        expires_at: new Date(Date.now() + 15 * 60_000).toISOString(),
      });
      return { user_code: 'SMOKE-CODE', verification_url: 'https://auth.openai.com/codex/device' };
    case 'poll_codex_signin':
      (globalThis.__SMOKE_SIGNIN_CALLS__ ??= []).push(\`poll_codex_signin:\${args.provider}\`);
      globalThis.__SMOKE_PENDING_SIGNINS__ = (globalThis.__SMOKE_PENDING_SIGNINS__ ?? []).filter((p) => p.provider_key !== args.provider);
      return 'complete';
    case 'cancel_signin':
      (globalThis.__SMOKE_SIGNIN_CALLS__ ??= []).push(\`cancel_signin:\${args.provider}\`);
      globalThis.__SMOKE_PENDING_SIGNINS__ = (globalThis.__SMOKE_PENDING_SIGNINS__ ?? []).filter((p) => p.provider_key !== args.provider);
      return null;
// Encrypted credential export/import (issue #151).
    case 'export_credential_bundle':
      (globalThis.__SMOKE_SHELL_CALLS__ ??= []).push('export_credential_bundle');
      return null;
    case 'import_credential_bundle':
      (globalThis.__SMOKE_SHELL_CALLS__ ??= []).push('import_credential_bundle');
      return globalThis.__SMOKE_IMPORT_REPORT__ ?? { accounts: {} };
    case 'set_dialog_open':
      (globalThis.__SMOKE_SHELL_CALLS__ ??= []).push(\`set_dialog_open:\${args.open}\`);
      return null;
    // Credential export/import (issue #152). The dialog stub (below) decides
    // which file the user "picked"; these stand in for the seal/apply commands
    // and record the exact arguments the component handed over, so a case can
    // prove the picked URI and the typed passphrase reach the command, that a
    // report renders, and that a refusal surfaces without touching the config.
    case 'export_credentials':
      (globalThis.__SMOKE_TRANSFER_CALLS__ ??= []).push(\`export_credentials:\${args.destination}:\${args.passphrase}\`);
      if (globalThis.__SMOKE_EXPORT_ERROR__) throw new Error(globalThis.__SMOKE_EXPORT_ERROR__);
      return null;
    case 'import_credentials':
      (globalThis.__SMOKE_TRANSFER_CALLS__ ??= []).push(\`import_credentials:\${args.source}:\${args.passphrase}\`);
      if (globalThis.__SMOKE_IMPORT_ERROR__) throw new Error(globalThis.__SMOKE_IMPORT_ERROR__);
      return globalThis.__SMOKE_IMPORT_REPORT__ ?? { accounts: {} };
    // Desktop→phone QR transfer (issue #156). Desktop's qr_transfer_frames
    // stands in for sealing + rendering; the mobile trio stand in for the
    // FrameCollector, tracked by the per-case __SMOKE_QR_STATUS_SEQUENCE__ so
    // a case can prove the scan loop keeps calling qr_scan_frame until it
    // reports complete, and only then reaches qr_scan_finish.
    case 'qr_transfer_frames':
      (globalThis.__SMOKE_TRANSFER_CALLS__ ??= []).push(\`qr_transfer_frames:\${args.passphrase}\`);
      if (globalThis.__SMOKE_QR_TRANSFER_ERROR__) throw new Error(globalThis.__SMOKE_QR_TRANSFER_ERROR__);
      return globalThis.__SMOKE_QR_FRAMES__ ?? ['<svg data-smoke-frame="0"></svg>'];
    case 'qr_scan_reset':
      (globalThis.__SMOKE_TRANSFER_CALLS__ ??= []).push('qr_scan_reset');
      globalThis.__SMOKE_QR_STATUS_INDEX__ = 0;
      return null;
    case 'qr_scan_frame': {
      (globalThis.__SMOKE_TRANSFER_CALLS__ ??= []).push(\`qr_scan_frame:\${args.text}\`);
      const seq = globalThis.__SMOKE_QR_STATUS_SEQUENCE__ ?? [{ have: 1, total: 1, complete: true }];
      const i = globalThis.__SMOKE_QR_STATUS_INDEX__ ?? 0;
      globalThis.__SMOKE_QR_STATUS_INDEX__ = Math.min(i + 1, seq.length - 1);
      return seq[i];
    }
    case 'qr_scan_finish':
      (globalThis.__SMOKE_TRANSFER_CALLS__ ??= []).push(\`qr_scan_finish:\${args.passphrase}\`);
      if (globalThis.__SMOKE_QR_FINISH_ERROR__) throw new Error(globalThis.__SMOKE_QR_FINISH_ERROR__);
      return globalThis.__SMOKE_IMPORT_REPORT__ ?? { accounts: {} };
    // LAN desktop pairing (issue #154). The five commands stand in for the
    // Rust session: generate returns a fixed code a case can trace into
    // receive_start, address returns the receiver's ready-to-type strings,
    // and the recorded calls prove the typed code/address reach the sender
    // command, that arming consumed the code the receiver shows, and that a
    // cancel reached the shell.
    case 'lan_pairing_address':
      return globalThis.__SMOKE_LAN_ADDRESS__ ?? [];
    case 'lan_pairing_generate_code':
      (globalThis.__SMOKE_TRANSFER_CALLS__ ??= []).push('lan_pairing_generate_code');
      return '428194';
    case 'lan_pairing_send':
      (globalThis.__SMOKE_TRANSFER_CALLS__ ??= []).push(\`lan_pairing_send:\${args.code}:\${args.address}\`);
      if (globalThis.__SMOKE_LAN_SEND_ERROR__) throw new Error(globalThis.__SMOKE_LAN_SEND_ERROR__);
      // A stalled receiver: the command stays pending until the shell's
      // cancel aborts it, which surfaces as a rejection — the same shape the
      // real command's oneshot produces.
      if (globalThis.__SMOKE_LAN_SEND_HANG__) {
        return new Promise((_, reject) => { globalThis.__SMOKE_LAN_SEND_REJECT__ = reject; });
      }
      return null;
    case 'lan_pairing_receive_start':
      (globalThis.__SMOKE_TRANSFER_CALLS__ ??= []).push(\`lan_pairing_receive_start:\${args.code}\`);
      if (globalThis.__SMOKE_LAN_START_ERROR__) throw new Error(globalThis.__SMOKE_LAN_START_ERROR__);
      return null;
    case 'lan_pairing_cancel':
      (globalThis.__SMOKE_TRANSFER_CALLS__ ??= []).push('lan_pairing_cancel');
      // Aborting the sender rejects its pending command, as the shell does.
      globalThis.__SMOKE_LAN_SEND_REJECT__?.(new Error('Transfer cancelled — nothing was sent.'));
      globalThis.__SMOKE_LAN_SEND_REJECT__ = null;
      return null;
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
  // host.js imports this lazily for credential export/import (issue #152),
  // the same pattern as the updater below. The per-case globals decide what
  // the system dialog "returned": null is a cancelled dialog.
  mkdirSync(join(WORK, '@tauri-apps/plugin-dialog'), { recursive: true });
  w('@tauri-apps/plugin-dialog/index.js', `
export async function save() { return globalThis.__SMOKE_DIALOG_SAVE__ ?? null; }
export async function open() { return globalThis.__SMOKE_DIALOG_OPEN__ ?? null; }`);
  // host.js imports this lazily for the desktop→phone QR transfer's camera
  // side (issue #156), same lazy-import pattern as the dialog plugin above.
  // __SMOKE_QR_SCAN_QUEUE__ hands back one { content } per call, in order,
  // so a case can drive a multi-frame scan loop deterministically; running
  // off the end throws, standing in for a cancelled scan.
  mkdirSync(join(WORK, '@tauri-apps/plugin-barcode-scanner'), { recursive: true });
  w('@tauri-apps/plugin-barcode-scanner/index.js', `
export async function checkPermissions() { return globalThis.__SMOKE_QR_PERMISSION__ ?? 'granted'; }
export async function requestPermissions() { return globalThis.__SMOKE_QR_PERMISSION__ ?? 'granted'; }
export async function cancel() { (globalThis.__SMOKE_QR_SCAN_QUEUE__ ??= []).length = 0; }
export async function scan() {
  const queue = globalThis.__SMOKE_QR_SCAN_QUEUE__ ?? [];
  if (queue.length === 0) throw new Error('scan cancelled');
  return { content: queue.shift(), format: 'QR_CODE', bounds: null };
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
  // Settings imports `save`/`open` statically for the encrypted credential
  // export/import controls (issue #151). The first host.js line below already
  // wrote the stub — this block is a no-op kept only for the explanatory
  // comment. The per-case globals __SMOKE_DIALOG_SAVE__ / __SMOKE_DIALOG_OPEN__
  // decide what the picker returned (null = cancelled).
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
  // Settings imports plugin-dialog statically (unlike the updater above), so
  // it needs the same bare-specifier rewrite the `@tauri-apps/api/*` rule
  // above does.
  code = code.replace(/(from\s+')@tauri-apps\/plugin-dialog(')/g, (_m, a, z) => {
    const target = relative(dirname(rel), `@tauri-apps/plugin-dialog/index.js`);
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
mkdirSync(join(WORK, 'src/mobile'), { recursive: true });
// Every plain module in these dirs, not a named list: a component importing a
// helper this harness forgot to stage fails to render for a reason that has
// nothing to do with the component. `src/mobile` carries the mobile shell's
// own helpers (e.g. the credential-test lifecycle MobileApp imports).
for (const dir of ['src/lib', 'src/mobile']) {
  for (const f of readdirSync(join(ROOT, dir)).filter((f) => f.endsWith('.js') && !f.endsWith('.test.js'))) {
    const rel = `${dir}/${f}`;
    const code = rewriteTauriImports(readFileSync(join(ROOT, rel), 'utf8'), rel);
    writeFileSync(join(WORK, rel), code);
  }
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
    globalThis.__SMOKE_CONFIG__ = c.config ? { snapshots: c.snapshots ?? SNAPSHOTS, config: c.config } : undefined;
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
    globalThis.__SMOKE_PENDING_SIGNINS__ = c.pendingSignins ?? [];
    globalThis.__SMOKE_SIGNIN_CALLS__ = [];
    globalThis.__SMOKE_TRANSFER_CALLS__ = [];
    globalThis.__SMOKE_DIALOG_SAVE__ = null;
    globalThis.__SMOKE_DIALOG_OPEN__ = null;
    globalThis.__SMOKE_IMPORT_REPORT__ = null;
    globalThis.__SMOKE_IMPORT_ERROR__ = '';
    globalThis.__SMOKE_EXPORT_ERROR__ = '';
    globalThis.__SMOKE_QR_FRAMES__ = c.qrFrames ?? null;
    globalThis.__SMOKE_QR_TRANSFER_ERROR__ = '';
    globalThis.__SMOKE_QR_PERMISSION__ = c.qrPermission ?? 'granted';
    globalThis.__SMOKE_QR_SCAN_QUEUE__ = [];
    globalThis.__SMOKE_QR_STATUS_SEQUENCE__ = null;
    globalThis.__SMOKE_QR_STATUS_INDEX__ = 0;
    globalThis.__SMOKE_QR_FINISH_ERROR__ = '';
    globalThis.__SMOKE_LAN_ADDRESS__ = c.lanAddress ?? ['192.168.1.20:45454'];
    globalThis.__SMOKE_LAN_SEND_ERROR__ = '';
    globalThis.__SMOKE_LAN_START_ERROR__ = '';
    app = mount((await import(build(c.file))).default, { target, props: c.props($) });
    flushSync();
    await new Promise((r) => setTimeout(r, 60)); // let onMount's awaits settle
    flushSync();
    // `emit` plays a Rust event to the component's own listeners; `shellCalls`
    // is what it asked Rust to do in return. Together they cover the navigation
    // seam in both directions without reaching into component internals.
    const shellCalls = () => globalThis.__SMOKE_SHELL_CALLS__;
    const sizes = () => globalThis.__SMOKE_SIZES__;
    const signinCalls = () => globalThis.__SMOKE_SIGNIN_CALLS__;
    await c.verify?.({ target, flushSync, emit, shellCalls, sizes, signinCalls });
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
