# AGENTS.md — quota-widget

Guidance for coding agents working in this repo. Read this fully before editing.

## What this is

A system-tray widget for **Windows 11 and Linux** showing AI provider quotas
(Claude 5-hour + weekly windows, Codex weekly, OpenRouter credits, Hermes Portal
credits). Tauri 2 (Rust) + Svelte 5. ~3,600 lines total — small enough to read
end to end, and you should.

```
crates/quota-core/   pure Rust: config, providers, alerts, model. Has all the tests.
src-tauri/           Tauri shell: tray, poller, secrets, OAuth, IPC commands.
src/                 Svelte 5 frontend (App, Settings, ProviderCard, HoverSummary).
nix/package.nix      Linux packaging. Also emits the .desktop entry.
```

`README.md` is user-facing documentation and is kept accurate — update it when
behaviour changes. It is a reliable description of current behaviour, not
aspiration.

---

## Ground rules

**Do not `git push` unless explicitly asked.** Every push triggers a Windows CI
build that the repo owner budgets by hand. Commit locally and wait for explicit
authorization before pushing.

**Do not add a git remote, change remotes, or touch credentials.** There is a
repo-local credential helper reading `~/.gh_token`, deliberately configured with
an empty helper entry first to suppress a global gh-CLI helper belonging to a
*different* GitHub account. Leave all of it alone.

**Version is single-sourced** from the workspace `Cargo.toml`; `tauri.conf.json`,
`package.json` and `nix/package.nix` derive from it. `npm run check-versions`
verifies this. Update the version number for every change, bumping it in
`Cargo.toml` only.

**If you touch `package.json` dependencies**, the `npmDeps.hash` in
`nix/package.nix` must be regenerated or the Nix build breaks.

---

## Build and test

```sh
cargo test -p quota-core   # 19 tests, all pure Rust — this is your main feedback loop
npm run build              # vite build of the Svelte frontend
npm run check-versions     # version consistency across the four files
```

**Building `src-tauri` locally may not work.** It needs `clang` and `lld`, which
are not always installed on the dev machine. `cargo check -p quota-core` covers
the pure-Rust crate and is where most logic lives — lean on it. If the Tauri
crate cannot be compiled locally, say so plainly in your report rather than
claiming a change is verified. Do not push to get CI to check your work.

CI (`.github/workflows/build.yml`) runs the core tests on Linux and builds the
Windows portable EXE + NSIS installer.

---

## Architecture notes that are easy to get wrong

### Provider identity is a string, not an enum

`Config.providers` is `BTreeMap<String, ProviderConfig>` (`config.rs:63-77`) and
`UsageSnapshot.provider_id` is a `String`. There is **no `ProviderId` enum**.
Adapters are unit structs whose `id()` returns `&'static str`
(`providers/mod.rs:43-58`), and each adapter reads its own settings by string
literal, e.g. `provider_setting("claude", "auth_mode")` (`claude.rs:44`).

### Config has no versioning and fails silently

`Config::load` (`config.rs:123`) does `serde_json::from_str(&text).unwrap_or_default()`
— **any parse error silently discards the entire config**, and there's a test
asserting that behaviour (`config.rs:156-162`). `save()` is a non-atomic
`fs::write`. Forward-compat rests entirely on `#[serde(default)]`. Adding fields
is safe; renaming or re-keying anything is not, without a migration step that
does not currently exist.

### Known live bug — `codex_oauth` secret is unreadable

`secrets.rs:10` has a hardcoded allow-list:

```rust
const PROVIDERS: &[&str] = &["claude", "codex", "openrouter", "hermes", "claude_oauth"];
```

`"codex_oauth"` is missing, so `load_all` never surfaces it and
`Codex::stored_auth` (`codex.rs:110-115`) always returns `None` — the built-in
Codex device sign-in stores a token the poller can never read, while
`Settings.svelte:40` still reports "signed in" because it calls `secrets::get`
directly. Fix this if you touch the secrets layer.

Also note: the Windows keyring backend has **no enumeration API**, so any dynamic
set of secret keys must be derived from `Config.providers` keys, never discovered
from the store.

### Codex has no token refresh

Claude self-refreshes (`claude.rs:221-252`). Codex implements only initial device
auth — no refresh path. Any design that assumes a long-lived Codex session is
wrong.

---

## Platform constraints — verified, do not try to work around

These were checked against vendored crate source and upstream docs. If your
instinct is to "just fix" one of them, it is a dead end.

### 1. Linux trays deliver no hover events. Ever.

The StatusNotifierItem D-Bus spec exposes only `Activate(x,y)`,
`SecondaryActivate(x,y)`, `ContextMenu(x,y)` and `Scroll`. **There is no
enter/leave concept at any layer of the stack.** Worse, the current backend —
`tray-icon 0.24.2` uses libappindicator (`platform_impl/gtk/mod.rs:12`) —
delivers only menu activations, so the `TrayIconEvent::Click` / `Enter` / `Leave`
handlers at `tray.rs:108-128` are **dead code on Linux**. That is why
`tray.rs:90` sets `show_menu_on_left_click(cfg!(target_os = "linux"))`.

What Plasma shows when you hover its battery applet is the applet's own SNI
`ToolTip` **property**, rendered by Plasma — the app never learns a hover
happened. So a Linux hover peek means *publishing tooltip text and letting the
desktop draw it*. It cannot be a window we style or position.

libappindicator exposes neither a tooltip nor activation, so getting either
requires moving to a native SNI implementation (`ksni`).

### 2. Native Wayland cannot position or raise its own windows

`xdg-shell` has no self-positioning and no always-on-top. `tao`'s
`set_outer_position` (`linux/window.rs:457`) is effectively a no-op there. This
is documented at `README.md:149-164` with the upstream trackers (tao#1134,
tauri#3117), both labelled *upstream*.

**The chosen workaround is XWayland**, and it is already in place:
`nix/package.nix` emits a `.desktop` entry with `Exec=env GDK_BACKEND=x11 quota-widget`.
The `on_wayland()` probe (`lib.rs:220-227`) already honours that override.
Fractional scaling can look blurry under XWayland; that trade-off was accepted.

Do **not** add gtk-layer-shell or a compositor-specific protocol without asking.

### 3. Positioning ignores panels (a real bug worth fixing)

`place_near_tray` (`tray.rs:160-175`) uses `monitor.size()` — full monitor bounds
— despite a doc comment claiming work area. Tauri 2.11.5 does expose
`monitor.work_area()` (`tauri/src/window/mod.rs:96`). Using it is the fix for
popups landing under the panel.

---

## Current task

A full implementation plan lives at **`docs/plan-tray-accounts.md`** in this
repo. Read it before starting. Summary of intent:

1. **Remove the native tray tooltip on Windows only** (`tray.rs:136`) — it draws
   over the nicer custom hover-peek window. Keep publishing the text on Linux,
   where it becomes the SNI tooltip.
2. **Give Linux tray parity** by swapping to `ksni` — hover shows a
   Plasma-drawn tooltip, left-click toggles the popup, right-click opens the menu.
3. **Add a pin button** to the popup: pinned means it survives focus loss, stays
   always-on-top, and anchors just above the panel. Unpinned popups still hide on
   blur/Esc. Pin is per-session UI state, not persisted.
4. **Support multiple accounts per provider** (two Claude, two Codex, etc.) by
   splitting adapter identity into `kind()` / `id()` / `name()` and instantiating
   one adapter per config entry. **Each account gets a user-typed name** —
   "Work Claude", "Home Codex" — shown on the popup card, hover peek, tooltip and
   toasts. Note the plan keeps the account *key* immutable and separate from the
   editable *label*: renaming must only ever write `label`, because the key is
   load-bearing for secret names (the Windows keyring can't be enumerated to fix
   them up) and for alert-engine state.

Parts 1–3 are one coherent milestone. Part 4 is independent and can land
separately — it shares no code with the tray work beyond `poller.rs`.

Verification steps are in the plan. Manual testing on KDE Plasma matters here:
several behaviours cannot be unit-tested.

---

## Style

Match the surrounding code. This codebase has a consistent voice: comments
explain *why*, particularly where a platform quirk forced a decision (see
`tray.rs:87-89`, `secrets.rs`, `nix/package.nix`). Preserve that — a future
reader hitting the same constraint should find the reason in place rather than
rediscovering it. Keep `README.md`'s platform-differences table and Caveats
section accurate when behaviour changes.
