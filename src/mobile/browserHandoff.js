// Pure browser handoff (issue #160). Both mobile sign-in launch points
// (Claude start, Codex's "open browser" action) go through this one module
// instead of each carrying its own in-WebView `window.open` fallback — the
// branch that produced the original bug (issue #158): a silently-failed
// launch left the user stranded with no indication anything went wrong.
//
// The opener is an injected dependency (`@tauri-apps/plugin-opener`'s
// `openUrl` in production — see `getOpener` in `../lib/host.js`) so this
// stays testable under vitest without a WebView. A rejection propagates to
// the caller instead of falling back to `window.open`, so a missing or
// failing opener surfaces as a real error the UI can show inline.
export async function openExternal(url, { opener } = {}) {
  if (!opener) throw new Error('no external browser opener');
  await opener(url);
}
