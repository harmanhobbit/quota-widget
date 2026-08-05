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
src/                 Svelte 5 frontend (App, Settings, ProviderCard, MiniSummary).
nix/package.nix      Linux packaging. Also emits the .desktop entry.
```

`README.md` is user-facing documentation and is kept accurate — update it when
behaviour changes. It is a reliable description of current behaviour, not
aspiration.

---

## Ground rules

**Never push to `main`.** This is the one hard rule. `main` is what the repo
owner merges into deliberately. Work on a feature branch and let the owner
choose when to merge.

**Branch from the latest `main`, and keep branches short-lived.** Fetch first
and branch from `origin/main`, not from whatever the working tree happens to be
sitting on. Do not routinely stack one feature branch on another: a branch that
depends on unmerged work makes both harder to review and produces version
conflicts at merge time (see "Version conflicts between branches"). If work
genuinely cannot proceed without an unmerged branch, say so and ask.

**Branch-build badges are CI-driven.** `QUOTA_WIDGET_BRANCH` is the sole source
of the visible branch badge. Never hardcode or strip it: local, `main` and
tagged release builds correctly leave it empty, while dispatched branch builds
show their ref.

**Commit freely, at logical intervals.** Commits are cheap and local — make one
whenever a coherent unit of work is done and the tests you can run still pass,
rather than accumulating one enormous commit at the end. A self-contained
refactor is a good unit. This keeps changes reviewable and makes a bad step easy
to back out.

**Push the feature branch when work is complete.** After the implementation and
applicable checks are complete, push the branch so the finished change is
available for review. Never push `main`. A branch push runs the Linux test job
(1x minutes, ~30s), which is the point: the branch is known-green before anyone
merges it. Windows packaging does not run on branches.

**Do not add a git remote, change remotes, or touch credentials.** There is a
repo-local credential helper reading `~/.gh_token`, deliberately configured with
an empty helper entry first to suppress a global gh-CLI helper belonging to a
*different* GitHub account. Leave all of it alone.

**Version is single-sourced** from the workspace `Cargo.toml`; `tauri.conf.json`,
`package.json` and `nix/package.nix` derive from it. `npm run check-versions`
verifies this. When the version does change, it changes in `Cargo.toml` only.

**Do not bump the version on a feature branch.** Two branches that each bump
`Cargo.toml` conflict on the same line, and the second one merged silently
inherits a number that no longer describes it. Versioning is a release
decision, made once, after work is merged — see "Releases" below.

**If you touch `package.json` dependencies**, the `npmDeps.hash` in
`nix/package.nix` must be regenerated or the Nix build breaks.

---

## Releases

Versions are [Semantic Versioning](https://semver.org): `MAJOR.MINOR.PATCH`.
PATCH is a bug fix that changes no behaviour anyone relies on, MINOR adds a
capability (a new provider, a new setting) compatibly, MAJOR breaks an existing
config file or user-visible contract. Pre-1.0 this repo treats MINOR as its
feature unit and PATCH as its fix unit.

Feature branches carry **no** version number. A release is a deliberate,
separate act on `main` after the work is merged:

```sh
git switch main && git pull                 # merged work, no version bump yet
$EDITOR Cargo.toml                          # set version = "0.17.0"
cargo update -w --offline                   # propagate into Cargo.lock
npm run check-versions
git commit -am "Release 0.17.0"             # one commit, version change only
git tag -a v0.17.0 -m "Release 0.17.0"      # annotated, never lightweight
git push origin main --follow-tags
```

The tag is what ships. Pushing `v*.*.*` is the only thing that builds the
Windows portable EXE and NSIS installer and attaches them to a draft release;
CI first checks the tag matches `Cargo.toml` and refuses to package a
mislabelled tree. `gh workflow run build.yml --ref <ref>` still produces the
same Windows artifacts on demand without publishing a release — it spends the
2x-metered Windows budget, so only do it when asked.

**`release/<version>` branches are for stabilisation only.** If a release needs
fixes after the version commit but before the tag — or an old line needs a
backport — cut `release/0.17.0` from the version commit, land only fixes there,
tag from it, then merge it back into `main`. Do not use one as a place to
accumulate features; if nothing needs stabilising, tag `main` directly.

### Version conflicts between branches

Two branches that both touched `Cargo.toml` will conflict there, and the merged
result is usually wrong even when git can auto-resolve it. Since feature
branches no longer bump the version, this should only show up on older branches
predating the rule:

1. Resolve the conflict by taking `main`'s version verbatim — never the
   branch's, and never a hand-merged third number.
2. Drop any commit whose only content was the bump (`git rebase -i`), so the
   branch contributes just its change.
3. Run `cargo update -w --offline` so `Cargo.lock` matches the resolved
   `Cargo.toml`; a stale lockfile version is its own CI failure.
4. Pick the release number afterwards, on `main`, from what actually landed —
   if a branch you expected to be a PATCH merged alongside a feature, the
   release is a MINOR.

---

## Build and test

```sh
cargo test -p quota-core   # 34 tests, all pure Rust — this is your main feedback loop
npm run build              # vite build of the Svelte frontend
npm run check-versions     # version consistency across the four files
npm i -D jsdom --no-save && npm run smoke-mount   # does the UI actually render?
```

**Building `src-tauri` needs the dev shell.** On a bare checkout the GTK/WebKit
`-sys` crates fail in their build scripts at `pkg-config --libs gdk-3.0`, so the
whole crate is uncompilable. The flake's `devShells.default` supplies those
system libraries:

```sh
nix develop                          # or automatically, via direnv + .envrc
nix develop -c cargo check --workspace
```

With direnv installed (`direnv allow` once), `cd`ing into the repo enters that
shell and plain `cargo` works. Prefer local Linux builds over pushing: GitHub's
Windows runner bills at a **2x** minute multiplier against a 2,000-minute
monthly quota, and Linux catches nearly everything that isn't platform-specific.

Without nix, `cargo check -p quota-core` still covers the pure-Rust crate where
most logic lives — lean on it. **If the Tauri crate cannot be compiled in your
environment, say so plainly in your report rather than claiming a change is
verified.** Do not push to get CI to check your work.

CI (`.github/workflows/build.yml`) runs the core tests on Linux for every branch
push and PR. The Windows portable EXE + NSIS installer are release-only: a
`v*.*.*` tag or a manual dispatch, never an ordinary push.

### `npm run build` passing does NOT mean the UI works

This has now shipped two user-visible breakages in a row, both of which built
clean. Svelte 5 has runtime-only failure modes that compile without a warning
and then **throw during render**, and a component that throws mid-render leaves
the previously rendered DOM on screen. The app-level symptom is "the page
doesn't open" or "clicking the button changes the header but not the body" —
never a build error, never anything in the terminal.

Run `npm run smoke-mount` for any frontend change. It mounts every top-level
component under jsdom with the Tauri IPC stubbed and fails if one throws or
renders nothing. It needs `npm i -D jsdom --no-save` first — jsdom is
deliberately not a `package.json` dependency, because adding one forces an
`npmDeps.hash` regen in `nix/package.nix` (see Ground rules). The
`--conditions=browser` flag in the npm script is load-bearing: without it Node
resolves Svelte's server build and every mount dies with
`lifecycle_function_unavailable`.

When adding a component, add it to `CASES` in `scripts/smoke-mount.mjs` with
the props its real parent passes. If a prop comes from a parent's `$state`,
pass it through `$.proxy()` there — that distinction is exactly what broke
Settings, and a plain object will not reproduce it.

The two runtime traps that have actually bitten this repo:

**`structuredClone` on a `$state` proxy throws `DataCloneError`.** Anything
that has been through `$state` — including a prop a parent holds in `$state` —
is a proxy, and `structuredClone` refuses to clone it. Use `$state.snapshot`,
which is the proxy-aware deep clone. This is also what you must pass over IPC:
`invoke('set_config', { config: $state.snapshot(config) })`.

**`{@const}` compiles to a derived, and deriveds must not write state.** A
lazy-init helper like `oauth[id] ??= {...}` is a state write, so calling it
from `{@const}` throws `state_unsafe_mutation`. Keep template helpers pure and
create entries eagerly (`ensureFlows()` in `Settings.svelte` is the pattern);
the same applies to any function called from `$derived`.

Neither is caught by the compiler, `npm run build`, or CI. Both are caught by
`smoke-mount`.

---

## Architecture notes that are easy to get wrong

### Provider identity is a string, not an enum

`Config.providers` is `BTreeMap<String, ProviderConfig>` (`config.rs:93`, with
`ProviderConfig` at `config.rs:41`) and `UsageSnapshot.provider_id` is a
`String`. There is **no `ProviderId` enum**. The `Provider` trait
(`providers/mod.rs:50-53`) splits identity three ways: `kind()` is the
`&'static str` adapter family, while `id()` and `name()` return owned per-account
values. Adapters are instantiated one per config entry and hold their own key,
so settings are read by that key rather than a literal, e.g.
`provider_setting(key, "auth_mode")` (`claude.rs:47`).

### Config has no versioning and fails silently

`Config::load` (`config.rs:157`) does `serde_json::from_str(&text).unwrap_or_default()`
(`config.rs:160`) — **any parse error silently discards the entire config**, and
there's a test asserting that behaviour
(`missing_or_corrupt_file_yields_defaults`, `config.rs:194`). `save()`
(`config.rs:165`) writes to `config.json.tmp` and renames, so a torn write can't
corrupt an existing config. Forward-compat rests entirely on `#[serde(default)]`.
Adding fields is safe; renaming or re-keying anything is not, without a migration
step that does not currently exist.

### Secret keys are derived from config, never enumerated

The Windows keyring backend has **no enumeration API**, so the set of secret
keys must be derived from `Config.providers` keys rather than discovered from
the store. `load_all` (`secrets.rs:118`) does exactly that: it walks the config
and, for `claude`/`codex` accounts, additionally derives `oauth_key(key)`.
Validation is a predicate, `valid_key` (`secrets.rs:12`) — there is no hardcoded
provider allow-list, and adding one would break multi-account, whose keys are
user-generated (`claude#2`).

This is why account *keys* are immutable and separate from editable *labels*:
renaming an account must only ever write `label`, because the key is
load-bearing for secret names that cannot be enumerated to fix up afterwards.

### Codex has no token refresh

Claude self-refreshes (`refresh`, `claude.rs:245`). Codex implements only initial
device auth — `stored_auth` (`codex.rs:128`) reads a token but there is no
refresh path. Any design that assumes a long-lived Codex session is wrong.

---

## Platform constraints — verified, do not try to work around

These were checked against vendored crate source and upstream docs. If your
instinct is to "just fix" one of them, it is a dead end.

### 1. Linux trays deliver no hover events. Ever.

The StatusNotifierItem D-Bus spec exposes only `Activate(x,y)`,
`SecondaryActivate(x,y)`, `ContextMenu(x,y)` and `Scroll`. **There is no
enter/leave concept at any layer of the stack.** Worse, the current backend —
`tray-icon 0.24.2` uses libappindicator (`platform_impl/gtk/mod.rs:12`) —
delivers only menu activations, so the `TrayIconEvent::Click` handler in
`create_tray` is **dead code on Linux** — which is why the
Linux tray is now a separate `ksni` implementation in `tray_linux.rs`, gated
`#[cfg(target_os = "linux")]` at `lib.rs:6`. The items in `tray.rs` carry the
matching `#[cfg(not(target_os = "linux"))]`, so that path is Windows/macOS only
and sets `show_menu_on_left_click(false)` unconditionally (`tray.rs:92`).

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
The `on_wayland()` probe (`lib.rs:271-277`) already honours that override.
Fractional scaling can look blurry under XWayland; that trade-off was accepted.

Do **not** add gtk-layer-shell or a compositor-specific protocol without asking.

### 3. Positioning respects panels — keep it that way

`place_near_tray` (`tray.rs:172`) uses `monitor.work_area()`, not
`monitor.size()`, so popups clamp inside the panel-excluded area rather than
landing under the panel. The mini-summary placement (`tray.rs:209`) does the
same. Both must keep using `work_area()`; switching to `size()` reintroduces the
under-panel bug.

---

## Current task

The active plan is **`docs/plan-updates-and-providers.md`**. Read it before
starting. It covers two tracks:

1. **Finesse patches.** Cosmetic only: rounded corners on the mini and main
   windows, mini-summary row alignment and percentage-first labels, a
   content-fitted mini height, and a collapsed "Add account" form. All shipped.
2. **Feature minors.** Branch builds with a visible dev badge, a
   Tailscale-vs-plain-SSH transport toggle, scroll-to-fade window opacity,
   upstream update detection, native Windows install, a Nix-aware update
   prompt, and new provider adapters.

**Numbering is a hard requirement:** finesse items are patch bumps, features are
minor bumps, and **no revision introduces more than one feature**. The plan
deliberately does *not* assign version numbers to unshipped features — each
takes the next available minor when it is built, read from `Cargo.toml` at that
moment. Pre-assigned numbers went stale once (the plan's "0.9.0" was spent on a
provider) and are not to be reintroduced. Task ownership between the Claude and
Codex workstreams is defined in the plan; keep to it, since the split exists to
stop two agents editing the same file.

Verification steps are in the plan. Manual testing on KDE Plasma and Windows 11
matters here — several behaviours cannot be unit-tested, and the corner-radius
work is entirely visual.

`docs/plan-tray-accounts.md` is the **previous, completed** plan. It remains a
good record of why tray, multi-account, `ksni`, and pinning work look the way
they do, but it is not a to-do list — do not reimplement from it.

---

## Style

Match the surrounding code. This codebase has a consistent voice: comments
explain *why*, particularly where a platform quirk forced a decision (see
`tray.rs:126`, `tray_linux.rs:1-2`, `secrets.rs`, `nix/package.nix`). Preserve that — a future
reader hitting the same constraint should find the reason in place rather than
rediscovering it. Keep `README.md`'s platform-differences table and Caveats
section accurate when behaviour changes.
