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
//
// Scrolling *up* fades and *down* restores. That is the opposite of the
// original plan, and deliberate: the gesture reads as pushing the window back
// into the desktop rather than as scrolling a document.
//
// Which physical gesture that *is* depends on the platform: the webview hands
// us the OS's own wheel sign, so the same flick fades on Linux and restores on
// Windows. `invert` flips the mapping so the user can match their machine.
export function stepOpacity(deltaY, minimum = 0, invert = false) {
  if (!Number.isFinite(deltaY) || deltaY === 0) return false;
  const direction = invert ? -1 : 1;
  const next = Math.min(1, Math.max(minimum, level + Math.sign(deltaY) * direction * STEP));
  if (next === level) return false;
  level = next;
  write();
  return true;
}
