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
  version: 1,
  poll_interval_secs: 60,
  autostart: false,
  hide_on_blur: false,
  mini_summary_bars: true,
  thresholds: { warn_pct: 80, critical_pct: 95 },
  alerts: { toast: true, tray_color: true, auto_popup: false },
  providers: {
    claude: provider({ enabled: true, mini_summary_metric: 'window:five_hour' }),
    // The unavailable weekly selection must fall back to this returned 5-hour window.
    codex: provider({ enabled: true, mini_summary_metric: 'window:weekly' }),
    openrouter: provider({}),
    hermes: provider({ mini_summary_metric: 'window:monthly_allowance' }),
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
      { metric_id: 'five_hour', label: '5h', used_pct: 42, informational: false },
      { metric_id: 'weekly', label: 'Weekly', used_pct: 88, informational: false },
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
];

// Every component under test, with the props App would really pass. Settings
// takes its config as a prop that has been through $state in App, so it
// arrives as a proxy — mirror that exactly, since it is what broke it before.
const CASES = [
  { file: 'src/App.svelte', props: () => ({}), buildBranch: 'smoke-branch', expect: ['smoke-branch'] },
  // Verifies a chosen lower 5-hour headline wins over Automatic's 88% weekly
  // value, while an unavailable selected Codex weekly falls back to its 5-hour value.
  { file: 'src/lib/MiniSummary.svelte', props: () => ({}), expect: ['42% 5h', '70% 5h', '3.42 USD', '100% Monthly allowance (Plus)'] },
  { file: 'src/lib/MiniSummary.svelte', props: () => ({}), snapshotsError: true, expect: ['Could not load summary'] },
  { file: 'src/lib/MiniSummary.svelte', props: () => ({}), buildBranch: 'smoke-branch', expect: ['smoke-branch'] },
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), snapshots: structuredClone(SNAPSHOTS), onclose() {} }),
    expect: ['Providers', 'Mini-summary headline', 'Automatic', 'None', 'Thresholds', 'Alerts', 'Save'],
    verify: async ({ target, flushSync }) => {
      const findButton = (text) => [...target.querySelectorAll('button')].find((button) => button.textContent.trim() === text);
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
      [...target.querySelectorAll('.provider')]
        .find((card) => card.querySelector('strong').textContent.trim() === 'OpenRouter')
        .querySelector('.provider-footer button').click();
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
  },
];

function stubTauri() {
  mkdirSync(join(WORK, '@tauri-apps/api'), { recursive: true });
  const w = (p, s) => writeFileSync(join(WORK, p), s);
  w('@tauri-apps/api/core.js', `
export async function invoke(cmd, args) {
  switch (cmd) {
    case 'get_snapshots': if (globalThis.__SMOKE_SNAPSHOTS_ERROR__) throw new Error('IPC unavailable'); return ${JSON.stringify({ snapshots: SNAPSHOTS, config: CONFIG })};
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
  'MutationObserver', 'SVGElement', 'Text', 'Comment', 'DocumentFragment']) {
  globalThis[k] = dom.window[k];
}
globalThis.__QUOTA_WIDGET_VERSION__ = '0.0.0-test';
globalThis.__QUOTA_WIDGET_BRANCH__ = '';

rmSync(WORK, { recursive: true, force: true });
stubTauri();

const { mount, unmount, flushSync } = await import('svelte');
const $ = await import('svelte/internal/client');

let failed = 0;
for (const c of CASES) {
  const target = dom.window.document.createElement('div');
  dom.window.document.body.appendChild(target);
  let app;
  try {
    globalThis.__SMOKE_SNAPSHOTS_ERROR__ = Boolean(c.snapshotsError);
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
  target.remove();
}

rmSync(WORK, { recursive: true, force: true });
if (failed) {
  console.error(`\nsmoke-mount: ${failed} component(s) failed to render`);
  process.exit(1);
}
console.log('\nsmoke-mount: all components mounted and rendered');
