import { defineConfig } from 'vite';
import { readFileSync } from 'node:fs';
import { svelte } from '@sveltejs/vite-plugin-svelte';

// Cargo.toml is the only version source. Embed it in the frontend so the
// visible title bar identifies the exact bundled build without an IPC call.
const cargoToml = readFileSync(new URL('./Cargo.toml', import.meta.url), 'utf8');
const appVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!appVersion) throw new Error('Could not read workspace version from Cargo.toml');

export default defineConfig({
  plugins: [svelte()],
  define: {
    __QUOTA_WIDGET_VERSION__: JSON.stringify(appVersion),
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
