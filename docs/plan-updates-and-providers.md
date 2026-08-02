# quota-widget roadmap: finesse patches (0.5.23–0.5.27), then features (0.6.0+)

## Context

The app is functionally complete for its original scope but has accumulated
visible rough edges, and there is no way to learn that a newer build exists.
Today updating means noticing by hand and replacing the EXE, and testing a
change on Windows means merging to `main` — the only branch CI builds.

This plan covers two tracks:

1. **Finesse (patch versions).** Five cosmetic defects, all in the popup and
   mini-summary chrome. None change behaviour.
2. **Features (minor versions).** Branch builds with a visible dev badge,
   update detection, native Windows install, a Nix-aware update prompt, a
   Tailscale-vs-plain-SSH transport toggle, and new provider adapters.

**Numbering rule (per Ian):** finesse items are patch bumps, features are minor
bumps, and **no revision introduces more than one feature**. Version lives only
in the workspace `Cargo.toml`; run `cargo update -w` and
`npm run check-versions` after every bump (`AGENTS.md`, "Ground rules").

**Ordering rationale.** Branch builds land first (0.6.0) because they are what
lets Ian test every later feature on Windows without merging to `main`. Update
detection (0.7.0) is the foundation both the Windows installer (0.8.0) and the
Nix prompt (0.9.0) build on. Tailscale (0.10.0) and the providers (0.11.0+) are
independent and can be reordered freely.

---

## Decisions taken (from Ian)

| Question | Decision |
|---|---|
| Repo visibility | **Stays private.** A separate *public* repo carries the manifest and release binaries. |
| Windows install | **Switch to the NSIS installer** for in-app updating; CI keeps emitting the portable EXE too. |
| New providers | **Hand-written built-in adapters**, not a generic config-driven one. |

### Which providers made the cut

Only providers with a **documented** balance/usage endpoint are in scope, which
is Ian's stated criterion. Confirmed: **DeepSeek**, **Moonshot/Kimi**,
**Anthropic Admin API**, **OpenAI Admin API**, **Fireworks**.

Dropped for now: **Z.AI**, **DeepInfra**, **Together**, **Groq**, **Mistral** —
each exposes spend through a dashboard rather than a documented API endpoint.
They are not planned; if one publishes an endpoint later it becomes a new minor
at that point. See [0.11.0+](#0110--one-provider-per-minor).

Separately: DeepSeek and Moonshot are ~95% identical to the existing
`openrouter.rs` (108 lines: GET a URL with a bearer token, pluck two numbers).
Rather than three near-copies I recommend one `providers/simple_credits.rs`
holding a static table of `(kind, display name, url, parse fn)`. This is **not**
the generic config-driven adapter that was rejected — from the user's side each
provider is still a named dropdown entry with a baked-in URL where you paste
only a key. It is purely an internal deduplication, and it is worth doing at
0.11.0 rather than retrofitting later.

---

## Track 1 — Finesse patches

These are strictly sequential (each is its own version bump), so shared files
like `src/styles.css` are not a parallelism hazard here.

### 0.5.23 — Mini-summary follows its own corner radius

`src/styles.css:42-44` already gives `body` a `1px` border and `border-radius:
10px`, but the `mini` window is opaque (`tauri.conf.json:39`,
`"transparent": false`), so the OS paints a square window and the rounded
corners have nothing to reveal.

- `src-tauri/tauri.conf.json` — set `"transparent": true` on the `mini` window
  (leave `"shadow": true`).
- `src/styles.css` — make `html, body` background transparent, and move
  `background`/`border`/`border-radius` onto the `.mini` wrapper so the rounded
  shape is what actually paints. Add `overflow: hidden` there so children can't
  square off the corners.

Platform notes, both already verified in vendored source: on Linux `tao` selects
an RGBA visual and sets `app_paintable` when `transparent` is set
(`tao-0.35.3/src/platform_impl/linux/window.rs:131,139`), which needs a running
compositor — Plasma qualifies, including under the XWayland launcher. On
Windows 11 transparent + undecorated + shadow is the documented route to rounded
corners; a dark-theme quirk is [reported upstream](https://github.com/tauri-apps/tauri/issues/13859),
so check both themes.

**Owner: Claude** (owns `tauri.conf.json`, tray/mini runtime).

### 0.5.24 — Main window follows its own corner radius

Same root cause, separate window and separate risk profile — `main` is
`resizable: true`, has no `shadow`, and hosts the drag regions.

- `src-tauri/tauri.conf.json` — `"transparent": true` and `"shadow": true` on
  `main`.
- `src/styles.css` — move the border/radius/background from `body` onto `main`
  (`src/App.svelte:79`), with `overflow: hidden`.

Verify `data-tauri-drag-region` still drags (`src/App.svelte:80`) and that the
resize edges still grab. Kept separate from 0.5.23 so either can be reverted
alone.

**Owner: Claude.**

### 0.5.25 — Mini-summary row alignment and percentage-first

Two changes to the same grid, so one patch. From the screenshot: names hug the
left edge and values hug the right, leaving a ragged gutter on both sides of the
bars.

- `src/styles.css` — `.hover-name { text-align: right }` and
  `.hover-val { text-align: left }` (currently `right`, at `styles.css:306`).
  The three-column grid at `styles.css:273-279` already lines the columns up;
  only the alignment within them changes.
- `src/lib/MiniSummary.svelte:68` — swap `windowSummary` to emit
  `${used_pct}% ${label}` instead of `${label} ${used_pct}%`.

These compose deliberately: percentage-first plus left-aligned values puts every
number in one scannable column immediately after the bars.

`scripts/smoke-mount.mjs:103` asserts the old strings (`'5h 42%'`, etc.) and
must be updated to `'42% 5h'` and friends, or the smoke test will fail.

**Owner: Claude.**

### 0.5.26 — Mini-summary height fits its content

The `mini` window is a fixed 300×140 (`tauri.conf.json:31-32`). Four rows
measure roughly 116px, which is the visible dead space at the bottom.

`App.svelte` solves this for the main window with `fitToContent()`
(`src/App.svelte:22-34`) calling `win.setSize()` directly — but the `mini`
capability deliberately grants no window-management permissions
(`src-tauri/capabilities/mini.json`: *"Rust does all positioning and
always-on-top work"*), and `core:window:allow-set-size` is not in `core:default`
(note `default.json` lists it explicitly for `main`).

Keep that boundary. Add a Rust command instead:

- `src-tauri/src/lib.rs` — `set_mini_height(height: f64)`, guarded on
  `window.label() == "mini"`, clamped to a sane range, registered in
  `generate_handler!`.
- `src-tauri/src/tray.rs` — the command must resize **and then** re-run
  `anchor_above_panel` (`tray.rs:200`) in one step. The window is anchored to
  the work area's *bottom* edge, so changing the height moves the top edge;
  without an atomic re-anchor the summary visibly jumps.
- `src/lib/MiniSummary.svelte` — mirror App's `$effect` + `tick()` pattern,
  measuring the rendered `.mini` element and invoking the command whenever
  `snapshots` changes.

**Risk to verify on Plasma:** the window is `resizable: false`, and tao maps a
resize to a bare `window.resize(w, h)`
(`tao-0.35.3/src/platform_impl/linux/event_loop.rs:302`) while GTK pins
non-resizable windows via geometry hints. If GTK ignores it, the fallback is
`"resizable": true` in `tauri.conf.json` with Rust still driving both size and
position.

**Owner: Claude.**

### 0.5.27 — Collapsed, roomier "Add account" form

`src/lib/Settings.svelte:269` puts the kind `<select>`, the name `<input>`, and
the Add button in one flex `.row`. The input is `flex: 1; min-width: 0`
(`styles.css:432-443`) and the select is `min-width: 0` (`styles.css:450-461`),
so in a 380px window the select is squeezed until "Claude" truncates.

Ian's preferred shape ("even better"): keep it collapsed behind a single
**+ Add account** button; clicking reveals a stacked form.

- `src/lib/Settings.svelte` — a `let addingAccount = $state(false)` toggle. The
  expanded panel stacks *name input on top*, then kind select and the
  confirm/cancel buttons below, reusing the existing `.field` class
  (`styles.css:471-484`), which is already the "stacked label-above-control,
  never overflows the narrow window" pattern. Reuse `addAccount()`
  (`Settings.svelte:206`) unchanged and collapse the panel on success.
- `src/styles.css` — a `.add-account` block; give the controls a larger touch
  target than the `.small` buttons around them.

`scripts/smoke-mount.mjs:109-131` drives this form (`.settings > section > .row
input` then clicking "Add account") and must be updated to open the panel first,
or the Settings case fails.

**Owner: Codex** (owns `Settings.svelte` / `App.svelte` per the established
split in `docs/plan-tray-accounts.md`).

---

## Track 2 — Feature minors

### 0.6.0 — Branch builds with a visible dev badge

**What already works:** `build.yml` has `workflow_dispatch`, and GitHub lets you
pick any ref for a dispatch, so `gh workflow run build.yml --ref my-branch`
builds a branch today. The dispatch also already bypasses the paths filter
(`build.yml:58`). Pushing a non-`main` branch triggers nothing (`build.yml:5`).

**What's missing:** the resulting EXE is indistinguishable from a `main` build,
and the artifact names collide.

- `.github/workflows/build.yml` — export `QUOTA_WIDGET_BRANCH: ${{ github.ref_name }}`
  to the build step, but only when `github.ref_name != 'main'`, so release
  builds stay unbadged. Suffix both `upload-artifact` names with the branch.
- `vite.config.js` — add a `__QUOTA_WIDGET_BRANCH__` define from
  `process.env.QUOTA_WIDGET_BRANCH ?? ''`, mirroring the existing
  `__QUOTA_WIDGET_VERSION__` plumbing (`vite.config.js:13`).
- `src-tauri/build.rs` — add `cargo:rerun-if-env-changed=QUOTA_WIDGET_BRANCH`
  so Rust can read it via `option_env!` without a stale-cache surprise. Rust
  needs it too: 0.7.0 must not nag a branch build about updates.
- `src/App.svelte:81` and `src/lib/MiniSummary.svelte:79` — render the branch
  next to the existing `v{APP_VERSION}` when the define is non-empty.
- `src/styles.css` — a `.build-branch` chip, visually distinct from
  `.build-version` (`styles.css:65-69`).
- **`AGENTS.md`** — this is the rule Ian asked for. Two parts: (a) the badge is
  driven solely by `QUOTA_WIDGET_BRANCH`, must never be hardcoded or stripped,
  and local/`main` builds correctly show nothing; (b) revise the existing "Do
  not `git push`" ground rule, whose stated cost ("every push triggers a Windows
  CI build") is no longer true for branches — pushing a branch is now free and
  the *dispatch* is the metered step.

**Owner: Claude** (build plumbing; the App.svelte touch is a two-line badge, not
worth splitting).

### 0.7.0 — Upstream update detection

Private source, public distribution — per Ian's decision.

**Ian-only manual steps (I must not do these):** create the public repo
`harmanhobbit/quota-widget-dist`; run `npm run tauri signer generate` and keep
the private key; add `TAURI_SIGNING_PRIVATE_KEY` and a `DIST_REPO_TOKEN` with
write access to the dist repo as Actions secrets on the private repo.
`AGENTS.md` forbids agents touching remotes or credentials.

- `.github/workflows/release.yml` (new) — on tag push or dispatch: build, then
  `gh release create` **against the dist repo**, uploading the installer, its
  `.sig`, and a generated `latest.json`. The app reads the stable URL
  `https://github.com/harmanhobbit/quota-widget-dist/releases/latest/download/latest.json`.
- `crates/quota-core/src/update.rs` (new) — `is_newer(current, candidate)` over
  parsed `(major, minor, patch)`, plus the `latest.json` deserializer. Logic
  belongs in quota-core because that is the crate with tests
  (`AGENTS.md`, "Build and test"); cover equal, older, newer, and malformed
  versions.
- `crates/quota-core/src/config.rs` — `check_updates: bool` (default `true`).
  Adding a `#[serde(default)]` field is the documented-safe change
  (`AGENTS.md`, "Config has no versioning").
- `src-tauri/src/lib.rs` — an `update: RwLock<Option<UpdateInfo>>` on
  `AppState`, checked once at startup and every ~6 h (not per poll — that would
  hammer GitHub), plus `check_update_now` and `update_status` IPC commands and
  an `update` event. **Suppress the check entirely when `option_env!("QUOTA_WIDGET_BRANCH")`
  is set** — a branch build should not be told to "update" to a main release.
- `src/App.svelte`, `src/lib/Settings.svelte` — an unobtrusive "Update
  available: vX.Y.Z" line plus a manual **Check now** button and the opt-out
  checkbox in the General section (`Settings.svelte:447-464`).
- `README.md` — document the dist repo and the opt-out.

**Owner: Codex** (core crate, config, Settings) with **Claude** on the workflow
and the `AppState`/IPC wiring.

### 0.8.0 — Native Windows update

Builds directly on 0.7.0's manifest.

- `src-tauri/Cargo.toml` — add `tauri-plugin-updater`, registered in
  `lib.rs`'s builder chain alongside the existing plugins
  (`src-tauri/src/lib.rs:348-353`).
- `src-tauri/tauri.conf.json` — `bundle.createUpdaterArtifacts: true`,
  `plugins.updater.pubkey`, `endpoints`, and `windows.installMode: "passive"`.
  Keep `installMode: "currentUser"` on the NSIS bundle
  (`tauri.conf.json:56-60`) so no admin prompt appears.
- `src-tauri/capabilities/default.json` — add `updater:default`. **Not**
  `mini.json`: the mini summary must not gain install rights.
- `src/lib/Settings.svelte` — an **Install update** button that calls
  `downloadAndInstall`, with progress. Note the plugin exits the app during the
  install step on Windows; say so in the UI before starting.
- `README.md` — the plugin cannot update a portable EXE in place. CI still
  publishes both artifacts; in-app updating requires having run the installer
  once. Say this plainly in the Deployment section.

**Owner: Claude** (Tauri shell, capabilities, `tauri.conf.json`).

### 0.9.0 — Nix-aware update prompt

0.7.0 detects the update everywhere; this makes the *instruction* correct per
install method, and gives Nix the prompt Ian asked for.

- `src-tauri/src/lib.rs` — classify the install: `std::env::current_exe()`
  starting with `/nix/store/` means a Nix build. Expose it through the existing
  `update_status` payload rather than a new command.
- `src/lib/Settings.svelte` / `src/App.svelte` — Nix shows the exact command
  (`nix profile upgrade quota-widget`) as selectable text, reusing the
  `.note code` style already used for `GDK_BACKEND=x11`
  (`styles.css:217-227`); non-Nix Linux shows a release link; Windows shows
  0.8.0's install button.
- `README.md` — an update matrix beside the existing platform-differences table.

**Owner: Codex.**

### 0.10.0 — Tailscale SSH vs plain SSH per connection

Currently `run_ssh` (`crates/quota-core/src/providers/hermes.rs:158`) always
builds `ssh -o BatchMode=yes -o ConnectTimeout=5 <host> <cmd>`.

**Load-bearing finding, verified against the local `tailscale` 1.98.10 CLI:**
`tailscale ssh` **rejects flags before the host** — `tailscale ssh -o
BatchMode=yes host` fails with `flag provided but not defined: -o` — but
**passes everything after the host straight through to the real `ssh`**
(`tailscale ssh host -o BogusOption=1` produced OpenSSH's own
`Bad configuration option`). So the argv must be *reordered*, not just have the
program name swapped:

```
plain:     ssh -o BatchMode=yes -o ConnectTimeout=5 <host> <cmd>
tailscale: tailscale ssh <host> -o BatchMode=yes -o ConnectTimeout=5 <cmd>
```

- `crates/quota-core/src/providers/hermes.rs` — a `transport` provider setting
  (`"ssh"` default | `"tailscale"`), read via the existing
  `provider_setting(key, …)` helper (`config.rs:158`). Branch the argv
  construction in `run_ssh`. Add a `tailscale_program` override alongside the
  existing `ssh_program` (`hermes.rs:164-168`) for Nix store paths — same
  reasoning as `refresh_cmd`. Keep `CREATE_NO_WINDOW` on Windows
  (`hermes.rs:173-174`).
- Tests — extend the existing stub-script pattern (`hermes.rs:633-654`) with a
  fake `tailscale` asserting the reordered argv. This is the whole reason the
  ordering detail is worth a test.
- `src/lib/Settings.svelte` — a Transport select in the Hermes block
  (`Settings.svelte:377-412`), shown only when the source is not `cookie`/`hermes`.
- `nix/package.nix` — add `tailscale` to the `makeBinPath` list in `preFixup`
  (`package.nix:84-89`), exactly as `openssh` is handled today.
- `README.md` — the Hermes row's remote-SSH note.

Note the `tailscale ssh` *server* is Linux/macOS-only, but the widget is always
the client, and the client subcommand exists on Windows.

**Owner: Codex** (core crate + Settings).

### 0.11.0+ — One provider per minor

Each new adapter is its own minor, honouring the one-feature-per-revision rule.
Every provider here has a documented endpoint; the two plain balance APIs come
first because they are the smallest and share the most code.

| Version | Provider | Endpoint | What it returns |
|---|---|---|---|
| 0.11.0 | DeepSeek | `GET https://api.deepseek.com/user/balance` | `balance_infos[]` with `total_balance`, `granted_balance`, `topped_up_balance`, and currency (CNY or USD) |
| 0.12.0 | Moonshot / Kimi | `GET https://api.moonshot.ai/v1/users/me/balance` | `available_balance`, `cash_balance`, `voucher_balance`. Keys are platform-specific — a `platform.kimi.ai` key 401s against `.com`, so make the base URL an overridable setting |
| 0.13.0 | Anthropic Admin | `GET /v1/organizations/cost_report` | Daily cost buckets. Needs an `sk-ant-admin-*` key, not a normal API key |
| 0.14.0 | OpenAI Admin | `GET /v1/organization/costs` | Daily cost buckets. Needs an admin key |
| 0.15.0 | Fireworks | `GET /v1/accounts/{account_id}/billingUsage` | Usage/cost. Needs an account id alongside the key |

Anthropic, OpenAI, and Fireworks report *spend over a period* rather than a
remaining balance, so they surface as a cost figure (and, where a budget is
configured, a percentage window) — not as `Credits`. Worth confirming that
framing when 0.13.0 comes up.

Per-provider work, the same shape every time:

- `crates/quota-core/src/providers/simple_credits.rs` (new, at 0.11.0) — the
  shared table described above, covering DeepSeek and Moonshot. Anthropic,
  OpenAI, and Fireworks do **not** fit it (time-bucketed cost reports needing
  date ranges, and Fireworks needs an account id in the path), so they get
  their own files.
- `crates/quota-core/src/providers/mod.rs` — register the kind in
  `adapter_kinds()` (`mod.rs:58`) and `providers_for()` (`mod.rs:69`).
- `src/lib/Settings.svelte` — add to the `PROVIDERS` array
  (`Settings.svelte:8-13`) and `metricOptions()` (`Settings.svelte:249-263`).
  These are hand-maintained mirrors of the Rust registry; missing either makes
  the provider unselectable.
- **Constraint:** kind strings must satisfy `secrets::valid_key`
  (`src-tauri/src/secrets.rs:12`) — ASCII alphanumeric plus `#_-` only, since
  the kind becomes the default account key and therefore the secret name. A dot
  or slash would silently break secret storage, so use e.g. `anthropic_admin`,
  never `anthropic.admin`.

**Owner: Codex** (all of it is core crate + Settings).

---

## Ownership summary

The split follows the boundary already established in
`docs/plan-tray-accounts.md`, which exists to keep two agents out of each
other's files:

- **Claude** — `src-tauri/**` (tray, capabilities, `tauri.conf.json`, IPC
  wiring), `src/lib/MiniSummary.svelte`, `src/main.js`, `.github/workflows/**`,
  `vite.config.js`.
- **Codex** — `crates/quota-core/**` (config, model, providers, tests),
  `src/lib/Settings.svelte`, `src/App.svelte`, `nix/package.nix`.
- **Shared, coordinate before editing** — `src/styles.css`,
  `scripts/smoke-mount.mjs`, `README.md`, `AGENTS.md`, `Cargo.toml`.

The patch series is strictly sequential, so ownership there is just *who does
the work*. Real parallelism is available across features: Codex can take
0.10.0 and the provider minors while Claude works 0.6.0–0.8.0.

Also worth doing during this work: `AGENTS.md`'s "Current task" section still
points at `docs/plan-tray-accounts.md`, which is finished. Repoint it here.

---

## Verification

Per release, before the version bump:

```sh
cargo test -p quota-core                          # main feedback loop
cargo check -p quota-core
npm run build
npm run check-versions                            # after cargo update -w
npm i -D jsdom --no-save && npm run smoke-mount   # required for ANY frontend change
```

`npm run build` passing does **not** mean the UI renders — `CLAUDE.md` and
`AGENTS.md` both call this out, and it has shipped two breakages already. Every
patch in Track 1 touches the frontend, so `smoke-mount` is mandatory for all of
them. Watch the two known Svelte 5 traps: `structuredClone` on a `$state` proxy
(use `$state.snapshot`), and state writes inside `{@const}`/`$derived`.

Building `src-tauri` locally may fail for want of `clang`/`lld`. Where it can't
be compiled, say so plainly rather than claiming verification.

Manual checks that cannot be automated:

- **0.5.23 / 0.5.24** — rounded corners on Plasma *and* Windows 11, in both
  light and dark themes; drag and resize still work on `main`.
- **0.5.26** — no dead space at the bottom for 1, 4, and 8 accounts; the
  summary does not jump when it resizes.
- **0.6.0** — a dispatched branch build shows the badge; a `main` build does
  not.
- **0.7.0 / 0.8.0** — an older build detects a newer `latest.json`; the
  installer path completes and relaunches; a branch build stays silent.
- **0.10.0** — both transports fetch Hermes against a real tailnet host.

Do not push, and do not dispatch a Windows build, without Ian saying so.
