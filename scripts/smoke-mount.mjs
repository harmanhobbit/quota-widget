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
    claude: provider({ enabled: true }),
    codex: provider({ enabled: true }),
    openrouter: provider({}),
    hermes: provider({}),
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
    windows: [{ label: '5h', used_pct: 42, informational: false }],
  },
];

// Every component under test, with the props App would really pass. Settings
// takes its config as a prop that has been through $state in App, so it
// arrives as a proxy — mirror that exactly, since it is what broke it before.
const CASES = [
  { file: 'src/App.svelte', props: () => ({}) },
  { file: 'src/lib/HoverSummary.svelte', props: () => ({}) },
  { file: 'src/lib/MiniSummary.svelte', props: () => ({}) },
  {
    file: 'src/lib/Settings.svelte',
    props: ($) => ({ initialConfig: $.proxy(structuredClone(CONFIG)), onclose() {} }),
    expect: ['Providers', 'Thresholds', 'Alerts', 'Save'],
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
export async function invoke(cmd) {
  switch (cmd) {
    case 'get_snapshots': return ${JSON.stringify({ snapshots: SNAPSHOTS, config: CONFIG })};
    case 'app_version': return '0.0.0-test';
    case 'has_secret': return false;
    case 'on_wayland': return true;
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
    app = mount((await import(build(c.file))).default, { target, props: c.props($) });
    flushSync();
    await new Promise((r) => setTimeout(r, 60)); // let onMount's awaits settle
    flushSync();
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
  target.remove();
}

rmSync(WORK, { recursive: true, force: true });
if (failed) {
  console.error(`\nsmoke-mount: ${failed} component(s) failed to render`);
  process.exit(1);
}
console.log('\nsmoke-mount: all components mounted and rendered');
