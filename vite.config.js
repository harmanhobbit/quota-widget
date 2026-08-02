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

export default defineConfig({
  plugins: [svelte()],
  define: {
    __QUOTA_WIDGET_VERSION__: JSON.stringify(appVersion),
    __QUOTA_WIDGET_BRANCH__: JSON.stringify(buildBranch),
  },
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    target: 'chrome110',
    outDir: 'dist',
  },
});
