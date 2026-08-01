import { mount } from 'svelte';
import App from './App.svelte';
import HoverSummary from './lib/HoverSummary.svelte';
import './styles.css';

// The tray hover peek is a second Tauri window pointed at ?view=hover. It
// shares this bundle but mounts a compact read-only summary instead of the
// full popup.
const isHover = new URLSearchParams(location.search).get('view') === 'hover';
const target = document.getElementById('app');
if (isHover) document.body.classList.add('hover-window');

export default mount(isHover ? HoverSummary : App, { target });
