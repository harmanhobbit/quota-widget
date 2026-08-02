import { mount } from 'svelte';
import App from './App.svelte';
import MiniSummary from './lib/MiniSummary.svelte';
import './styles.css';

// The tray-click mini summary is a second Tauri window pointed at ?view=mini.
// It shares this bundle but mounts a compact summary instead of the full
// popup. Tray hover is the shell's own native tooltip on both platforms, so
// there is no third window for it.
const isMini = new URLSearchParams(location.search).get('view') === 'mini';
const target = document.getElementById('app');

// The two windows differ in whether the OS paints their background: `mini` is a
// transparent window whose rounded shape is drawn by `.mini` itself, so the
// document behind it must not paint an opaque square over the corners. CSS is
// shared between both windows, so the distinction needs a hook on the root.
if (isMini) document.documentElement.classList.add('mini-view');

export default mount(isMini ? MiniSummary : App, { target });
