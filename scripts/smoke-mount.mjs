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
import { readFileSync, writeFileSync, mkdirSync, rmSync } from 'node:fs';
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

// Every component under test, with the props App would really pass. Settings
// takes its config as a prop that has been through $state in App, so it
// arrives as a proxy — mirror that exactly, since it is what broke it before.
const CASES = [
  {
    file: 'src/App.svelte',
    props: () => ({}),
    buildBranch: 'smoke-branch',
    expect: ['smoke-branch'],
    verify: ({ target, flushSync }) => {
      const main = target.querySelector('main');
      const opacity = () => document.documentElement.style.getPropertyValue('--window-opacity');
      main.dispatchEvent(new window.WheelEvent('wheel', { deltaY: -100, bubbles: true, cancelable: true }));
      flushSync();
      if (opacity() !== '0.92') throw new Error(`scroll did not fade popup: ${opacity()}`);
      // A config broadcast turns the toggle off and restores the normal shell.
      // The event stub does not retain listeners, so exercise the disabled
      // configuration directly through the initial IPC payload in its own case.
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
  // Claude shows both selected headlines as separate rows; Codex's only
  // selected window is missing from its snapshot, so it reads "no data";
  // OpenRouter is Automatic and falls through to its credit balance.
  {
    file: 'src/lib/MiniSummary.svelte',
    props: () => ({}),
    // Fireworks' labelled spend takes "Cost this month" as its row label,
    // where an unlabelled balance shows the bare currency.
    expect: ['42%', '5h', '88%', 'Weekly', 'no data', '$3.42', 'USD', '100%', 'Monthly allowance (Plus)', '$8.75', 'Cost this month'],
    verify: ({ target }) => {
      const names = [...target.querySelectorAll('.hover-name')].map((el) => el.textContent);
      // Claude's second row must leave the name blank rather than repeat it.
      if (names.slice(0, 3).join('|') !== 'Claude||Codex') {
        throw new Error(`name column was ${names.join('|')}`);
      }
      const bars = [...target.querySelectorAll('.hover-bar')];
      // Claude's 5-hour window is one hour into five, so its marker sits at 20%.
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
  {
    file: 'src/lib/MiniSummary.svelte',
    props: () => ({}),
    verify: ({ target, flushSync }) => {
      const mini = target.querySelector('.mini');
      const opacity = () => document.documentElement.style.getPropertyValue('--window-opacity');
      const wheel = () =>
        mini.dispatchEvent(new window.WheelEvent('wheel', { deltaY: -100, bubbles: true, cancelable: true }));
      wheel();
      flushSync();
      if (opacity() !== '0.92') throw new Error(`scroll did not fade summary: ${opacity()}`);
      // Unlike the popup the unpinned summary may reach zero: click-away
      // dismisses it, so fully transparent is recoverable rather than a trap.
      for (let i = 0; i < 20; i += 1) wheel();
      flushSync();
      if (Number(opacity()) !== 0) throw new Error(`unpinned summary floored at ${opacity()}`);
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
    },
  },
  {
    file: 'src/lib/ProviderCard.svelte',
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
    file: 'src/lib/ProviderCard.svelte',
    props: () => ({ snap: structuredClone(SNAPSHOTS[4]) }),
    expect: ['Cost this month: 8.75 USD'],
  },
  {
    file: 'src/lib/ProviderCard.svelte',
    props: () => ({ snap: structuredClone(SNAPSHOTS[2]) }),
    verify: ({ target }) => {
      const balance = target.querySelector('.balance').textContent.trim();
      if (balance !== '3.42 USD') throw new Error(`unlabelled balance rendered as ${balance}`);
    },
  },
];

function stubTauri() {
  mkdirSync(join(WORK, '@tauri-apps/api'), { recursive: true });
  const w = (p, s) => writeFileSync(join(WORK, p), s);
  w('@tauri-apps/api/core.js', `
export async function invoke(cmd, args) {
  switch (cmd) {
    case 'get_snapshots': if (globalThis.__SMOKE_SNAPSHOTS_ERROR__) throw new Error('IPC unavailable'); return globalThis.__SMOKE_CONFIG__ ?? ${JSON.stringify({ snapshots: SNAPSHOTS, config: CONFIG })};
    case 'app_version': return '0.0.0-test';
    case 'has_secret': return false;
    case 'on_wayland': return true;
    case 'set_config': globalThis.__SMOKE_LAST_CONFIG__ = args.config; return null;
    default: return null;
  }
}`);
  w('@tauri-apps/api/event.js', `export async function listen() { return () => {}; }`);
  w('@tauri-apps/api/window.js', `
export function getCurrentWindow() {
  return { setSize: async () => {}, outerPosition: async () => ({x:0,y:0}), scaleFactor: async () => 1 };
}`);
  w('@tauri-apps/api/dpi.js', `
export class LogicalSize { constructor(w,h){ this.width=w; this.height=h; } }
export class LogicalPosition { constructor(x,y){ this.x=x; this.y=y; } }`);
}

// Compile a component and its local .svelte imports, rewriting bare Tauri
// specifiers to the stubs above.
const built = new Set();
function build(rel) {
  if (built.has(rel)) return join(WORK, rel.replace(/\.svelte$/, '.js'));
  built.add(rel);
  const src = readFileSync(join(ROOT, rel), 'utf8');
  const { js } = compile(src, { generate: 'client', filename: rel });
  let code = js.code.replace(/(from\s+')@tauri-apps\/api\/([\w-]+)(')/g, (_m, a, mod, z) => {
    const target = relative(dirname(rel), `@tauri-apps/api/${mod}.js`);
    return `${a}${target.startsWith('.') ? target : './' + target}${z}`;
  });
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
writeFileSync(join(WORK, 'src/lib/opacity.js'), readFileSync(join(ROOT, 'src/lib/opacity.js'), 'utf8'));

const { mount, unmount, flushSync } = await import('svelte');
const $ = await import('svelte/internal/client');

let failed = 0;
for (const c of CASES) {
  const target = dom.window.document.createElement('div');
  dom.window.document.body.appendChild(target);
  let app;
  try {
    globalThis.__SMOKE_SNAPSHOTS_ERROR__ = Boolean(c.snapshotsError);
    globalThis.__SMOKE_CONFIG__ = c.config ? { snapshots: SNAPSHOTS, config: c.config } : undefined;
    globalThis.__QUOTA_WIDGET_BRANCH__ = c.buildBranch ?? '';
    app = mount((await import(build(c.file))).default, { target, props: c.props($) });
    flushSync();
    await new Promise((r) => setTimeout(r, 60)); // let onMount's awaits settle
    flushSync();
    await c.verify?.({ target, flushSync });
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
  target.remove();
}

rmSync(WORK, { recursive: true, force: true });
if (failed) {
  console.error(`\nsmoke-mount: ${failed} component(s) failed to render`);
  process.exit(1);
}
console.log('\nsmoke-mount: all components mounted and rendered');
