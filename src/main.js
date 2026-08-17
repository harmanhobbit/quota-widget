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
// Android's WebView reports its platform in the user agent; every desktop
// target (Windows/Linux, whatever the OS) does not. No window/tray concepts
// exist there, so it gets its own shell rather than branching App.svelte.
const isMobile = navigator.userAgent.includes('Android');
const target = document.getElementById('app');

export default mount(isMobile ? MobileApp : isMini ? MiniSummary : App, { target });
