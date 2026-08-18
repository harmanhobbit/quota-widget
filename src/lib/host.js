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
