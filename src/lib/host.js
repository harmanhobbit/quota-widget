// Small host API adapter. Desktop and mobile shells both call through this
// instead of `@tauri-apps/api` directly, so a shared component never assumes
// a desktop-only command exists — it only ever sees the operations named
// here. `invoke`/`listen` themselves already work identically across Tauri's
// desktop and Android targets; this module exists for the *command set*, not
// for platform detection.
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';

export { listen };

export const getSnapshots = () => invoke('get_snapshots');
export const setConfig = (config) => invoke('set_config', { config });
export const setSecret = (provider, value) => invoke('set_secret', { provider, value });
export const hasSecret = (provider) => invoke('has_secret', { provider });
export const clearSecret = (provider) => invoke('clear_secret', { provider });
export const refreshNow = () => invoke('refresh_now');
// The manual refresh (issue #111): durable one-time work on the native host,
// so the fetch survives the app being dismissed right after the tap. The
// foreground loop deliberately keeps using `refreshNow` — while the app is
// visible the fetch is supposed to run in-process, immediately.
export const refreshManual = () => invoke('refresh_manual');
export const testProvider = (provider) => invoke('test_provider', { provider });
export const startClaudeSignin = (provider) => invoke('start_claude_signin', { provider });
export const finishClaudeSignin = (provider, code) => invoke('finish_claude_signin', { provider, code });
export const startCodexSignin = (provider) => invoke('start_codex_signin', { provider });
export const pollCodexSignin = (provider) => invoke('poll_codex_signin', { provider });
export const cancelSignin = (provider) => invoke('cancel_signin', { provider });
export const getPendingSignins = () => invoke('get_pending_signins');

// The external-browser opener plugin (issue #159), through its proper JS
// binding rather than the `window.__TAURI__` global — that global is only
// injected when `app.withGlobalTauri` is set in tauri.conf.json, which this
// app does not set, so `window.__TAURI__?.opener?.openUrl` was always
// `undefined` and every mobile sign-in silently fell through to (and, after
// issue #160 removed that fallback, threw past) a dead branch. Bundler
// imports work regardless of that setting, matching `invoke`/`listen` above.
export const getOpener = () => openUrl;

// Credential export & import (issue #152). Only the user-picked file URI and
// the passphrase cross the bridge: the commands read and write the file
// in-process, so the webview never holds a filesystem permission of its own.
export const exportCredentials = (destination, passphrase) =>
  invoke('export_credentials', { destination, passphrase });
export const importCredentials = (source, passphrase) =>
  invoke('import_credentials', { source, passphrase });

// The system file dialogs come from the dialog plugin, imported lazily the
// same way Settings pulls in the updater plugin: under the smoke-mount
// harness only a dynamic `import('@tauri-apps/...')` specifier is rewritten
// to the stub, and a static import would resolve the real package, whose
// calls need a live WebView. `.qwb` is the sealed-bundle file extension both
// platforms' pickers filter on (the format's magic is `QWSB`).
export const pickExportDestination = async () =>
  (await import('@tauri-apps/plugin-dialog')).save({
    defaultPath: 'quota-widget-export.qwb',
    filters: [{ name: 'Quota Widget credential export', extensions: ['qwb'] }],
  });
export const pickImportSource = async () =>
  (await import('@tauri-apps/plugin-dialog')).open({
    multiple: false,
    directory: false,
    filters: [{ name: 'Quota Widget credential export', extensions: ['qwb'] }],
  });

// Desktop→phone QR transfer (issue #156). Desktop renders the frames;
// Android scans them. Both sides share this one module so a component never
// imports `@tauri-apps/plugin-barcode-scanner` directly — the lazy dynamic
// import keeps it out of the desktop bundle and lets `smoke-mount` rewrite
// the specifier to its stub, same as the dialog plugin above.
export const qrTransferFrames = (passphrase) => invoke('qr_transfer_frames', { passphrase });
export const qrScanReset = () => invoke('qr_scan_reset');
export const qrScanFrame = (text) => invoke('qr_scan_frame', { text });
export const qrScanFinish = (passphrase) => invoke('qr_scan_finish', { passphrase });

export const qrScan = async (options) =>
  (await import('@tauri-apps/plugin-barcode-scanner')).scan(options);
export const qrCancelScan = async () => (await import('@tauri-apps/plugin-barcode-scanner')).cancel();
export const qrCheckPermissions = async () =>
  (await import('@tauri-apps/plugin-barcode-scanner')).checkPermissions();
export const qrRequestPermissions = async () =>
  (await import('@tauri-apps/plugin-barcode-scanner')).requestPermissions();
