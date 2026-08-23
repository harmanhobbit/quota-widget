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
