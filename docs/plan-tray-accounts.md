# quota-widget: Linux tray parity, pinnable popup, multi-account

## Context

Three things are wrong or missing today:

1. **The native tray tooltip overlays the hover peek on Windows.** `tray.rs:136` calls
   `tray.set_tooltip(...)` on every poll while `show_hover()` (`tray.rs:142`) opens the
   custom `hover` window. Both fire on `TrayIconEvent::Enter`, so the OS tooltip draws
   over the nicer peek window. The peek is the keeper; the tooltip is the intruder.

2. **Neither works on Linux.** `tray-icon 0.24.2`'s Linux backend is libappindicator
   (`platform_impl/gtk/mod.rs:12`), which delivers menu activations and *nothing else* —
   no `Click`, no `Enter`, no `Leave`. So `tray.rs:108-128` is dead code on Linux, hence
   the `show_menu_on_left_click(cfg!(target_os = "linux"))` workaround at `tray.rs:90`.

3. **Only one account per provider.** Adapters are unit structs with `&'static str` ids
   (`providers/mod.rs:43-58`) and every adapter reads its settings by string literal, e.g.
   `provider_setting("claude", "auth_mode")` (`claude.rs:44`).

### Two hard constraints that shape the design

**Hover events do not exist in the tray protocol.** StatusNotifierItem exposes only
`Activate(x,y)`, `SecondaryActivate(x,y)`, `ContextMenu(x,y)` and `Scroll`. There is no
enter/leave concept at any layer. What Plasma shows when you hover the battery applet is
the applet's *own* SNI `ToolTip` property, rendered by Plasma — the app never learns a
hover happened. So on Linux the peek is a **published tooltip**, not a window we control:
we set the text, Plasma draws it. That is exactly the battery-applet behaviour asked for,
and it is reachable — but it requires leaving libappindicator, which exposes no tooltip
and no activation. **Swap the Linux tray to `ksni` (0.3.6)**, a native SNI implementation
that gives us `tool_tip()`, `activate(x, y)` and `context_menu(x, y)`.

**Native Wayland cannot position or raise its own windows.** `xdg-shell` has no
self-positioning and no always-on-top; `tao`'s `set_outer_position` (linux/window.rs:457)
is a no-op there. Already documented at `README.md:149-164` (tao#1134, tauri#3117). Per
decision: **ship under XWayland** — bake `GDK_BACKEND=x11` into the launcher so placement
and always-on-top work. The `on_wayland()` probe at `lib.rs:220-227` already honours that
override.

### Target behaviour

| | Windows 11 | Linux (KDE Plasma) |
|---|---|---|
| Hover | Custom peek window (tooltip removed) | Plasma-drawn SNI tooltip, same per-provider lines |
| Left-click | Toggle popup | Toggle popup |
| Right-click | Menu | Menu |
| Pin | Button top-right of popup | Same |

Pinned = stays through focus loss, always-on-top, anchored just above the panel.
Unpinned popups still vanish on blur/Esc. Pin is per-session UI state, not persisted.

---

## Part 1 — Tooltip and hover peek

**`src-tauri/src/tray.rs`**
- `set_status()` (`:133-138`): keep `set_icon`, drop `set_tooltip` on Windows only.
  Keep the text flowing on Linux, where it becomes the ksni tooltip (Part 2).
  `poller.rs:79-82` and `tooltip_line()` (`poller.rs:106-118`) stay as-is — the text is
  now consumed by two different sinks.
- Leave `show_hover`/`hide_hover` (`:142-156`) and the `hover` window
  (`tauri.conf.json:27-41`) untouched; they remain Windows-only.

**Verify while here:** `capabilities/hover.json` grants only `core:event:default`, but
`HoverSummary.svelte:9` calls `invoke('get_snapshots')`. Commit `648d00c` was fixing this —
confirm the grant covers invoke, else the peek is empty until the first poll push.

## Part 2 — Linux tray via ksni

New module **`src-tauri/src/tray_linux.rs`**, behind `#[cfg(target_os = "linux")]`.
`tray.rs` keeps the Windows path; `lib.rs:295` dispatches to one or the other.

- Add `ksni = "0.3"` (default tokio feature — Tauri already runs on tokio).
- A `QuotaTray` struct holding current `Status`, gauge `fill`, and the tooltip lines,
  implementing `ksni::Tray`:
  - `icon_pixmap()` — reuse `badge_icon()` (`tray.rs:14-63`) verbatim; it already produces
    raw RGBA. ksni wants ARGB32, so add a small channel-order shim rather than a second
    renderer.
  - `tool_tip()` — `description` = the newline-joined `tooltip_line()` output. This is the
    Plasma hover peek.
  - `activate(x, y)` — `toggle_popup(app, Some(PhysicalPosition::new(x, y)))`.
  - `context_menu()` / `menu()` — mirror the four existing items (`tray.rs:77-81`):
    Open / Refresh now / Settings / Quit, with the same handlers, including the
    show-then-`emit("navigate","settings")` ordering from `tray.rs:94-98`.
- `spawn()` returns a `Handle`; stash it in `AppState`. Linux `set_status` becomes
  `handle.update(|t| { t.status = …; t.fill = …; t.lines = … })`.
- Delete the `show_menu_on_left_click(cfg!(target_os = "linux"))` workaround — left-click
  is now the popup, right-click the menu, as on Windows.

**Launcher:** already done — `nix/package.nix` emits a `.desktop` entry with
`Exec=env GDK_BACKEND=x11 quota-widget` (commit `554daec`), and `README.md:149-164`
documents the fractional-scaling blur caveat. Nothing to change here; just don't
regress it.

**Note on `libayatana-appindicator`:** `nix/package.nix` lists it in `buildInputs` and
adds it to `LD_LIBRARY_PATH` in `preFixup` because the tray dlopens it at runtime. Once
the tray moves to ksni (pure D-Bus via zbus, no C library), that dependency and the
`LD_LIBRARY_PATH` prefix can be dropped — but only after confirming nothing else in the
GTK stack pulls it in.

## Part 3 — Pin button

**`src/App.svelte`** — add a pin toggle in the header next to `✕`, `let pinned = $state(false)`.
On toggle `invoke('set_pinned', { pinned })`. Show pinned state visually (filled vs outline).
Reset to unpinned when the window is hidden, so a fresh popup is always transient.

**`src-tauri/src/lib.rs`** — new `set_pinned` command:
- `state.hide_on_blur` (already an `AtomicBool`, consulted at `lib.rs:305`) is forced off
  while pinned and restored from config when unpinned. Keep the user's config setting
  separate from the pin override.
- `window.set_always_on_top(pinned)`.
- When pinning, reposition via a new `anchor_above_panel(&win)` helper in `tray.rs`,
  alongside `place_near_tray` (`:160-175`).

**`anchor_above_panel`** — the existing helper uses `monitor.size()` despite its doc
comment claiming work area, so it ignores panels. Use
`monitor.work_area()` (available in Tauri 2.11.5, `window/mod.rs:96`) and place the window
flush to the work-area bottom edge, horizontally near the tray x. Fix `place_near_tray` to
use `work_area()` too — that's the bug behind popups landing under the panel.

Esc handling (`App.svelte:49-54`) should no-op while pinned.

## Part 4 — Multiple accounts per provider

`Config.providers` is already `BTreeMap<String, ProviderConfig>` (`config.rs:63-77`) and
`UsageSnapshot.provider_id` is already `String`, so the container, the alert engine
(keyed `(provider_id, window_label)`, `alerts.rs:49`), the tray maths, `ProviderCard.svelte`
and `HoverSummary.svelte` all work unchanged with extra keys. The work is in the adapter
registry and the UI.

**Account key = config map key.** Existing keys (`claude`, `codex`, …) keep working
untouched — no re-keying, no migration. New accounts get `claude#work`-style keys.

**`crates/quota-core/src/config.rs`** — add two optional fields to `ProviderConfig`:
```rust
pub kind: Option<String>,   // adapter to use; defaults to the map key
pub label: Option<String>,  // user-facing account name; defaults to the adapter's name
```

### Account naming (user-editable)

Each account carries a **user-chosen display name**, typed freely by the user — e.g.
"Work Claude", "Home Codex", "Personal Claude". That name is what appears on the popup
card, the hover peek, the tooltip and any toast; the user should never see the internal
key. This is the whole point of multi-account — two cards both reading "Claude" would be
useless — so naming is a first-class part of the feature, not a nicety.

**Two identifiers, deliberately separate:**

| | Purpose | Mutable? |
|---|---|---|
| **Account key** (`BTreeMap` key, e.g. `claude#work`) | Config key, snapshot `provider_id`, secret-name prefix, alert-engine key | **No** — renaming would orphan secrets and reset alert state |
| **Label** (`ProviderConfig.label`) | Everything the user sees | **Yes** — freely editable any time |

Renaming an account must therefore only ever write `label`. Never re-key the map on
rename: the key is load-bearing across the secret store (which the Windows keyring cannot
enumerate to fix up), the edge-triggered alert engine (`alerts.rs:49`), and any stale
snapshot already in `state.snapshots`.

**Where the label surfaces.** `UsageSnapshot.provider_name` (`model.rs:64`) is already a
`String` and already reaches every display site, so this needs no new plumbing — just set
it from the label instead of the adapter's static name:

- `ProviderCard.svelte:42` — the popup card heading
- `HoverSummary.svelte:48` — the Windows hover peek row
- `poller.rs:117` / `:108` — tooltip lines (`"{name}: {…}"` / `"{name}: unavailable"`),
  which on Linux become the Plasma-drawn SNI tooltip
- `poller.rs:90-91` — toast alert titles (`"{name} — critical"`), via
  `AlertEvent.provider_name` (`alerts.rs:17`)

So `Provider::name()` returns the instance label, falling back to the adapter's built-in
name when `label` is unset. That fallback is what keeps existing single-account configs
rendering exactly as they do today.

**Rules for the Settings UI:**
- The name is a **free-text field**, not a picker — the user types "Work Claude" or
  "Home Codex" or anything else. Default it to `"{Kind} {n}"` (e.g. "Claude 2") so it is
  never blank, but expect it to be overwritten immediately.
- Trim whitespace; reject empty. Warn on duplicates but allow them — they're confusing,
  not corrupting, since the key is what's unique.
- **Keep labels short.** The hover peek window is 300 px wide (`tauri.conf.json:27-41`)
  and `.hover-row` (`styles.css:246`) lays out name / bar / value on one line. Long names
  will need ellipsis truncation — check `styles.css:230-295` renders sanely with a
  ~20-character name before calling this done.
- Derive the account key from the kind plus a slug or counter, not from the label. A label
  typed as "Ian's work 💼" must not become a config key or a keyring entry name.
Both `Option` + `#[serde(default)]`, so old configs parse identically. Add a
`version: u32` field now (currently absent) as insurance for future changes — noting
`load()` (`config.rs:123`) silently discards the whole config on any parse error, and
`save()` is a non-atomic `fs::write`; both are worth tightening in the same pass.

**`crates/quota-core/src/providers/mod.rs`** — the core change. Split identity:
```rust
fn kind(&self) -> &'static str;   // "claude" — which adapter
fn id(&self) -> &str;             // account key — snapshot id, config key, secret prefix
fn name(&self) -> &str;           // display label
```
Replace `all_providers()` with `providers_for(cfg: &Config) -> Vec<Box<dyn Provider>>`,
constructing one instance per config entry in map order. Keep a
`fn adapter_kinds() -> &[(&str, &str)]` listing the four kinds for the UI's "add account"
picker.

**Each adapter** (`claude.rs`, `codex.rs`, `openrouter.rs`, `hermes.rs`) — unit struct
becomes `pub struct Claude { pub key: String, pub label: Option<String> }`, and every
literal-keyed lookup takes the instance key instead:
`provider_setting("claude", "auth_mode")` → `provider_setting(&self.key, "auth_mode")`.
Same pattern at `codex.rs:30,78`, `openrouter.rs:22,27`, and the ~9 sites in `hermes.rs`.

**Secrets** (`src-tauri/src/secrets.rs`) — key names derive from the account key:
`claude` → `claude_oauth` (unchanged for existing installs), `claude#work` →
`claude#work_oauth`; bare key for OpenRouter/Hermes. The fixed `PROVIDERS` allow-list at
`secrets.rs:10` cannot enumerate dynamic keys — replace it with a charset/length
validation on the key. This also fixes a live bug: `codex_oauth` is missing from that list,
so `load_all` never surfaces it and `Codex::stored_auth` (`codex.rs:110-115`) always
returns `None` — the built-in Codex device sign-in stores a token the poller can never
read, while `Settings.svelte:40` still reports "signed in". The Windows keyring has no
enumeration API, so `load_all` must derive its key set from `Config.providers`.

**Sign-in plumbing** (`src-tauri/src/lib.rs`) — `oauth_pending`
(`lib.rs:26`, a single `Mutex<Option<PendingLogin>>`) becomes a
`Mutex<HashMap<String, PendingLogin>>` keyed by account key. `claude_oauth_start/finish`
(`lib.rs:139-170`) and the Codex device flow (`lib.rs:176-214`) take an account key and
write to the derived secret key instead of the hardcoded constants at `lib.rs:165,198`.
The `codex-oauth` event payload must carry the account key so the right UI row updates.

**CLI credential sources are inherently single-account** — one `~/.claude/.credentials.json`,
one `~/.codex/auth.json`. Offer the *CLI* and *Auto* auth modes only when the account key
equals the kind (the original account); additional accounts are built-in sign-in only.
Worth flagging: Claude self-refreshes (`claude.rs:221-252`) so extra accounts stay alive
indefinitely, but **Codex has no refresh implementation** — a second Codex account will
need periodic re-authentication until that's added.

**`get_snapshots`** (`lib.rs:56-70`) and the poller loop (`poller.rs:31-41`) switch from
`all_providers()` to `providers_for(&cfg)`; `test_provider`'s id match (`lib.rs:126-135`)
keys on `kind()`. Note the poller fetches **sequentially** — with N accounts that's N
serial round-trips per cycle, so make this pass concurrent (`join_all`) at the same time.

**`src/lib/Settings.svelte`** — the hardcoded `PROVIDERS` const (`:8-13`) becomes a list
derived from the config map. The per-provider `{#if}` islands (`:178`, `:211`, `:242`,
`:278`) switch from `p.id === 'claude'` to `p.kind === 'claude'`. The singleton OAuth UI
state (`:19-22`) and the fixed `has_secret` probes (`:39-40`) become maps keyed by account
key. Add an **Add account** control (pick a kind, then name it), a **Remove** per
non-default account, and an editable **name field** on every account row that writes
`label` only — see *Account naming* above for why the key must never be re-keyed on
rename. Removing an account must also clear its secrets, or the credentials linger in the
keyring/`secrets.json` with nothing referencing them.

---

## Suggested order

1. Part 1 + `work_area()` positioning fix — small, independently verifiable.
2. Part 2 ksni — biggest Linux unknown; land it before building the pin on top.
3. Part 3 pin.
4. Part 4 multi-account — largest diff, touches every layer, but the least risky
   architecturally since the config container is already a map.

## Verification

- `cargo test -p quota-core` — 19 existing tests must stay green. Add cases for:
  key→secret-name derivation (`claude` still maps to `claude_oauth`), `providers_for`
  building N instances from a config with two Claude entries, an old
  config.json (no `kind`/`label`/`version`) round-tripping unchanged, and
  **label fallback** — `label: None` yields the adapter's built-in name, `label: Some("Work")`
  yields "Work" in `UsageSnapshot.provider_name`.
- `cargo build` in `src-tauri` — needs `clang`/`lld` locally per the project notes; if it
  can't build here, CI covers it.
- `npm run build` for the Svelte side.
- Manual on Plasma: hover tray → tooltip lines appear (Plasma-drawn); left-click →
  popup toggles; right-click → menu; pin → survives clicking another window, sits above
  the panel, stays on top; unpin → blur-hides again.
- Manual multi-account: add a second Claude account, **name it "Work"**, sign in via the
  built-in flow, and confirm the name appears in all four surfaces — popup card, hover
  peek row, tooltip line, and any toast it fires. Confirm the tray colour reflects the
  worse of the two accounts and respects each one's *Include in tray icon*.
- Rename an account in Settings and confirm the label updates everywhere **without**
  losing its sign-in state — that's the check that proves rename didn't re-key the map.
- Windows: confirm via CI artifact that the OS tooltip is gone and the peek window is
  unobscured.

**Do not push** — per standing instruction, every push burns a Windows CI run. Commit
locally and let Ian choose when to build.
