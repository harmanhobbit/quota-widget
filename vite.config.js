import { defineConfig } from 'vite';
import { readFileSync } from 'node:fs';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Cargo.toml is the only version source. Embed it in the frontend so the
// visible title bar identifies the exact bundled build without an IPC call.
const cargoToml = readFileSync(new URL('./Cargo.toml', import.meta.url), 'utf8');
const appVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!appVersion) throw new Error('Could not read workspace version from Cargo.toml');
// CI supplies this only for branch builds. Keeping the empty default is what
// makes local and main builds visually identical to a release build.
const buildBranch = process.env.QUOTA_WIDGET_BRANCH ?? '';

// Which shell main.js mounts (desktop App vs mobile MobileApp) is decided at
// BUILD time, not by sniffing navigator.userAgent at runtime: under Tauri's
// wry Android shell the System WebView's UA did not reliably contain "Android"
// (it reported a desktop UA in CI), so the runtime check silently mounted the
// desktop app on the phone. `tauri android build` re-runs this build via its
// beforeBuildCommand with TAURI_ENV_PLATFORM=android|ios set, which is the
// authoritative signal. QUOTA_WIDGET_MOBILE is an explicit CI override the
// Android job also sets, so the proof never depends on Tauri internals alone.
const isMobileBuild =
  process.env.QUOTA_WIDGET_MOBILE === '1' ||
  process.env.TAURI_ENV_PLATFORM === 'android' ||
  process.env.TAURI_ENV_PLATFORM === 'ios';

export default defineConfig({
  plugins: [svelte()],
  define: {
    __QUOTA_WIDGET_VERSION__: JSON.stringify(appVersion),
    __QUOTA_WIDGET_BRANCH__: JSON.stringify(buildBranch),
    __IS_MOBILE__: JSON.stringify(isMobileBuild),
  },
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    // Downlevel to ES2019 so esbuild transforms ES2020+ syntax that older
    // WebViews can't parse: nullish coalescing (`??`), optional chaining
    // (`?.`), and logical-assignment (`??=`/`||=`/`&&=`, emitted by both our
    // code and Svelte 5's compiled runtime). Desktop WebViews (WebView2 /
    // WebKitGTK) parse the modern syntax fine, but Tauri's Android shell can
    // run on a System WebView old enough to throw "Unexpected token =" on the
    // whole bundle. `chrome110` here was an unexplained default, not a hard
    // requirement, so lowering it project-wide is safe.
    target: 'es2019',
    outDir: 'dist',
  },
});
