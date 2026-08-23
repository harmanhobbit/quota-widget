// Small host API adapter. Desktop and mobile shells both call through this
// instead of `@tauri-apps/api` directly, so a shared component never assumes
// a desktop-only command exists — it only ever sees the operations named
// here. `invoke`/`listen` themselves already work identically across Tauri's
// desktop and Android targets; this module exists for the *command set*, not
// for platform detection.
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

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

// The external-browser opener plugin (issue #159), read lazily so callers
// always see its current availability rather than a value captured at
// import time. Undefined when the plugin isn't registered — the caller (see
// `browserHandoff.js`'s `openExternal`) treats that as "no opener" and fails
// loudly instead of falling back to an in-WebView `window.open`.
export const getOpener = () => window.__TAURI__?.opener?.openUrl;
