import { mount } from 'svelte';
import App from './App.svelte';
import MiniSummary from './lib/MiniSummary.svelte';
import MobileApp from './mobile/MobileApp.svelte';
import './styles.css';
import './mobile/mobile.css';

// The tray-click mini summary is a second Tauri window pointed at ?view=mini.
// It shares this bundle but mounts a compact summary instead of the full
// popup. Tray hover is the shell's own native tooltip on both platforms, so
// there is no third window for it.
const isMini = new URLSearchParams(location.search).get('view') === 'mini';
// Mobile vs desktop is decided at BUILD time (see vite.config.js): the Android
// APK is built with TAURI_ENV_PLATFORM=android, baking __IS_MOBILE__ true, so
// it mounts the phone shell (MobileApp) instead of branching App.svelte. The
// userAgent check is only a runtime fallback: Tauri's wry Android WebView does
// NOT reliably put "Android" in navigator.userAgent (in CI it reported a
// desktop UA), which silently mounted the desktop app on the phone — the bug
// this replaces. No window/tray concepts exist on mobile, hence a separate
// shell.
const isMobile = __IS_MOBILE__ || navigator.userAgent.includes('Android');
const target = document.getElementById('app');

export default mount(isMobile ? MobileApp : isMini ? MiniSummary : App, { target });
