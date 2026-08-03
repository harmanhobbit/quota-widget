// Each webview has its own module instance, so this level is intentionally
// local to one window. The preference is persisted in Config; the temporary
// fade level is reset whenever the window is shown again.
const STEP = 0.08;
let level = 1;

function write() {
  document.documentElement.style.setProperty('--window-opacity', String(level));
}

export function resetOpacity() {
  level = 1;
  write();
}

// Returns whether the wheel event actually changed opacity. Callers use that
// to avoid swallowing ordinary scrolling once the fade has reached an edge.
export function stepOpacity(deltaY, minimum = 0) {
  if (!Number.isFinite(deltaY) || deltaY === 0) return false;
  const next = Math.min(1, Math.max(minimum, level - Math.sign(deltaY) * STEP));
  if (next === level) return false;
  level = next;
  write();
  return true;
}
