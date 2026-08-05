# quota-widget roadmap: finesse patches, then features

## Context

The app is functionally complete for its original scope but has accumulated
visible rough edges, and there is no way to learn that a newer build exists.
Today updating means noticing by hand and replacing the EXE, and testing a
change on Windows means merging to `main` — the only branch CI builds.

This plan covers two tracks:

1. **Finesse (patch versions).** Five cosmetic defects, all in the popup and
   mini-summary chrome. None change behaviour.
2. **Features (minor versions).** Branch builds with a visible dev badge, a
   Tailscale-vs-plain-SSH transport toggle, scroll-to-fade window opacity,
   update detection, native Windows install, a Nix-aware update prompt, and new
   provider adapters.

**Numbering rule (per Ian):** finesse items are patch bumps, features are minor
bumps, and **no revision introduces more than one feature**. Version lives only
in the workspace `Cargo.toml`; run `cargo update -w` and
`npm run check-versions` after every bump (`AGENTS.md`, "Ground rules").

**Features in this plan carry no version number.** Each one takes the next
available minor at the time it is built — read the current version out of
`Cargo.toml` and bump from there. Pre-assigning numbers here went wrong once
already: this plan had update detection pencilled in as 0.9.0, and by the time
anyone reached it, 0.9.0 had gone to the ElevenLabs provider, leaving every
number after it a lie. Ordering is what this plan asserts; numbering is decided
when the work lands. Shipped items below record the version they actually got,
as history rather than as a reservation.

**Ordering rationale.** Branch builds landed first because they are what lets
Ian test every later feature on Windows without merging to `main`. Tailscale
followed, being independent of everything else here. Scroll-to-fade opacity came
next by Ian's request; like Tailscale it blocks nothing and is blocked by
nothing. All three sat ahead of the update chain because that chain was gated on
manual steps only Ian could take (creating the dist repo, generating the signing
key, adding the Actions secrets), so putting unblocked work first kept things
moving while that was set up. That gate is now lifted — see the update-detection
section. Update detection is then the foundation both the
Windows installer and the Nix prompt build on — those three are a strict chain
and must ship in that order. The providers are independent of everything,
including each other, and can be reordered freely or interleaved with the update
chain.

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
at that point. See [One provider per minor](#one-provider-per-minor).

Separately: DeepSeek and Moonshot are ~95% identical to the existing
`openrouter.rs` (108 lines: GET a URL with a bearer token, pluck two numbers).
Rather than three near-copies I recommend one `providers/simple_credits.rs`
holding a static table of `(kind, display name, url, parse fn)`. This is **not**
the generic config-driven adapter that was rejected — from the user's side each
provider is still a named dropdown entry with a baked-in URL where you paste
only a key. It is purely an internal deduplication, and it is worth doing with
the first of those two providers rather than retrofitting later.

**Also shipped, ahead of this list: ElevenLabs** (0.9.0) — official
`GET /v1/user/subscription`, keyed by `xi-api-key`. It reports a per-cycle
credit *allowance* rather than a balance, so it renders as a `UsageWindow` like
Claude's weekly cap, not as `Credits`. It predates none of the table below; it
was simply asked for first.

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

### Branch builds with a visible dev badge — shipped as 0.6.0

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
  needs it too: update detection must not nag a branch build about updates.
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

**Owner: Codex** (build plumbing; the App.svelte touch is a two-line badge, not
worth splitting).

### Tailscale SSH vs plain SSH per connection — shipped as 0.7.0

Moved ahead of the update chain because it shares no files with it, and the
update work cannot start until the dist repo and signing secrets exist.

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
  `provider_setting(key, …)` helper (`config.rs:260`). Branch the argv
  construction in `run_ssh`. Add a `tailscale_program` override alongside the
  existing `ssh_program` (`hermes.rs:164-168`) for Nix store paths — same
  reasoning as `refresh_cmd`. Keep `CREATE_NO_WINDOW` on Windows
  (`hermes.rs:173-174`).
- Tests — extend the existing stub-script pattern (`hermes.rs:633-654`) with a
  fake `tailscale` asserting the reordered argv. This is the whole reason the
  ordering detail is worth a test.
- `src/lib/Settings.svelte` — a Transport select in the Hermes block
  (`Settings.svelte:496-531`), shown only when the source is not `cookie`/`hermes`
  (i.e. inside the existing `{#if}` at `Settings.svelte:505`).
- `nix/package.nix` — add `tailscale` to the `makeBinPath` list in `preFixup`
  (`package.nix:84-89`), exactly as `openssh` is handled today.
- `README.md` — the Hermes row's remote-SSH note.

Note the `tailscale ssh` *server* is Linux/macOS-only, but the widget is always
the client, and the client subcommand exists on Windows.

**Owner: Codex** (core crate + Settings).

### Scroll-to-fade window opacity — shipped as 0.8.0–0.8.2

Ian's request: with the pointer over the widget, scrolling down fades it all the
way to fully transparent and scrolling up returns it to fully opaque, with a
Settings toggle to disable the behaviour entirely.

**Direction reversed after trying it:** scrolling *up* fades and *down*
restores. On hardware the original way round felt backwards — the gesture reads
as pushing the window back into the desktop, not as scrolling a document.

**Load-bearing finding: do this in CSS, not through a window API.** Neither
`tao` 0.35.3 nor `tauri` 2.11.5 exposes a `set_opacity` on the window (grepped
both vendored trees; the symbol does not exist), so there is no native route in
the current dependency set. Fortunately none is needed — the corner-radius
patches already set `"transparent": true` on **both** windows
(`tauri.conf.json`), and the
visible shape is painted by a single element in each: `main` for the popup
(`styles.css:54-63`) and `.mini` for the summary (`styles.css:273-284`). Fading
that element's own background is genuinely see-through, not a fake blend against
an opaque window.

So the whole feature is one CSS custom property driving `opacity`, which also
makes it free on both platforms and instantly revertible.

- `src/styles.css` — add `--window-opacity: 1` to `:root`. Apply `opacity:
  var(--window-opacity)` to `main` and `.mini`. Prefer `opacity` on the shell
  element over `color-mix` on `background` alone: at low values the *text* must
  fade too, or a "fully transparent" widget still shows floating glyphs.
  Add a short `transition` so a scroll step glides rather than snaps.
- **A new `src/lib/opacity.js`** holding the shared logic: current level,
  a `step()` clamped to `[0, 1]`, and the write to
  `document.documentElement.style.setProperty('--window-opacity', …)`. Both
  windows need identical behaviour and they share no component — without this
  the logic gets copy-pasted into `App.svelte` and `MiniSummary.svelte`, which
  is exactly the drift the ownership split exists to prevent. **This file is the
  coordination point: it is new, so assign it to one owner and have the other
  import it.**
- `src/App.svelte` — an `onwheel` on `<main>` (`App.svelte:80`) calling the
  helper when the toggle is on. Must call `preventDefault()` only when it acts,
  or it will eat scrolling in the `.cards` list (`App.svelte:93`) and in
  Settings.
- `src/lib/MiniSummary.svelte` — the same handler on the `.mini` element, which
  is already bound as `miniEl`.
- `crates/quota-core/src/config.rs` — `scroll_opacity: bool` (default `true`),
  next to `mini_summary_bars` (`config.rs:108`). A `#[serde(default)]` field on
  a `#[serde(default)]` struct is the documented-safe config change
  (`AGENTS.md`, "Config has no versioning"); add it to the `Default` impl at
  `config.rs:139-141` too.
- `src/lib/Settings.svelte` — a checkbox in the General section beside the
  existing `mini_summary_bars` one (`Settings.svelte:578`). No new IPC is
  needed: `set_config` already broadcasts a `config` event (`lib.rs:119`) and
  `MiniSummary` already listens for it (`MiniSummary.svelte:33-36`), so the mini
  window picks the change up for free.

**Decisions worth confirming when it's built** — I've picked a default for each
rather than blocking:

- *Fully transparent means unclickable.* At `opacity: 0` the popup is invisible
  but still on top and still eating clicks, which reads as a frozen desktop.
  Plan: floor the popup at a low-but-visible value (~0.15) and treat 0 as
  reachable only on the mini summary, or pair 0 with click-through. Flag this
  early — it is the one way this feature can feel broken.
- *Persistence.* The level resets to 1 on reopen; only the on/off toggle is
  saved. Persisting the level means a config write per scroll tick, and a widget
  that reopens invisible is hard to recover from.
- *Granularity.* ~0.08 per wheel notch, so roughly a dozen notches spans the
  range — this is the number to tune by feel on hardware.

**Verification.** `smoke-mount` can drive this directly: dispatch a
`WheelEvent` at `main` and assert `--window-opacity` moved, then assert it does
*not* move with the toggle off. Do that — the wheel handler is exactly the kind
of frontend logic a clean `npm run build` will not catch. Real transparency
still needs eyes on Plasma (a compositor must be running) and Windows 11.

**Owner: Codex**, which keeps `config.rs`, `Settings.svelte`, and `App.svelte`
— three of the five files — with their established owner. Codex also creates
`src/lib/opacity.js`; **Claude** then imports it into `MiniSummary.svelte` and
takes the `styles.css` edit, coordinating as that file already requires. Codex
must land the helper before Claude's side starts.

### Upstream update detection

**Next available minor.** First of the three-step update chain; nothing below it
can start until this ships.

Private source, public distribution — per Ian's decision.

**Ian-only manual steps (I must not do these) — reported done by Ian, 2026-08-05:**
create the public repo `harmanhobbit/quota-widget-dist`; run
`npm run tauri signer generate` and keep the private key; add
`TAURI_SIGNING_PRIVATE_KEY` and a `DIST_REPO_TOKEN` with write access to the
dist repo as Actions secrets on the private repo. `AGENTS.md` forbids agents
touching remotes or credentials.

The dist repo is confirmed to exist and be public. Both secrets are present
under exactly the names above, confirmed by Ian from the repo's Actions
settings. They are not *agent*-verifiable — the `gh` login available to agents
here has no access to the private repo, so `gh secret list` 404s — so the only
remaining check is dispatching the release workflow: a wrong key *value* or a
token missing write scope on the dist repo surfaces there as a signing or
upload failure, and nothing local can catch it.

#### The `latest.json` contract — agree before either agent starts

This shape has **three** consumers written by two agents: the release workflow
generates it (Claude), `update.rs` parses it (Codex), and in the next minor
`tauri-plugin-updater` reads the very same file (Claude). Two agents guessing
independently is how this plan produced duplicate adapters once already, so the
schema is fixed here rather than discovered:

```json
{
  "version": "0.17.0",
  "notes": "See the release page for details.",
  "pub_date": "2026-08-05T09:23:00Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "<contents of the .sig file>",
      "url": "https://github.com/harmanhobbit/quota-widget-dist/releases/download/v0.17.0/QuotaWidget_0.17.0_x64-setup.exe"
    }
  }
}
```

This is **`tauri-plugin-updater`'s own documented format**, chosen deliberately:
the plugin in the next minor consumes this file unmodified, so inventing a
custom shape now would mean reshaping it later, mid-chain. `version` is bare
SemVer with no `v` prefix — the git tag keeps its `v`, the manifest does not.
`pub_date` is RFC 3339. `platforms` is keyed by Tauri's target triple form, and
today has exactly one key; adding Linux later must not require a schema change,
so parse it as a map, never as a fixed struct.

`UpdateInfo`, the type crossing from quota-core into `AppState` and out over
IPC, is the parsed subset the UI actually renders — `current`, `latest`, `url`,
`notes`, `pub_date` — plus whatever the "is an update available" verdict needs.
Codex owns its definition; Claude imports it and must not declare a parallel
copy in `src-tauri`. Land `update.rs` before the `AppState` wiring starts.

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
  This is why the branch-badge work had to land first.
- `src/App.svelte`, `src/lib/Settings.svelte` — an unobtrusive "Update
  available: vX.Y.Z" line plus a manual **Check now** button and the opt-out
  checkbox in the General section (`Settings.svelte:447-464`).
- `README.md` — document the dist repo and the opt-out.

**Owner: Codex** (core crate, config, Settings) with **Claude** on the workflow
and the `AppState`/IPC wiring.

### Native Windows update

**Next available minor after update detection**, which it builds directly on —
it consumes that feature's `latest.json` manifest and cannot ship before it.

- `src-tauri/Cargo.toml` — add `tauri-plugin-updater`, registered in
  `lib.rs`'s builder chain alongside the existing plugins
  (`src-tauri/src/lib.rs:348-353`).
- `src-tauri/tauri.conf.json` — `plugins.updater.pubkey`, `endpoints`, and
  `windows.installMode: "passive"`. **`bundle.createUpdaterArtifacts` already
  landed with update detection**, not here: the release workflow publishes a
  `latest.json` carrying a `signature`, and without that flag there is no `.sig`
  file to read, so the manifest would ship empty-signed from day one. The flag
  is inert until a signing key is present, so moving it earlier costs nothing.
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

### Nix-aware update prompt

**Next available minor after the Windows installer.** Update detection reports a
new version everywhere; this makes the *instruction* correct per install method,
and gives Nix the prompt Ian asked for.

- `src-tauri/src/lib.rs` — classify the install: `std::env::current_exe()`
  starting with `/nix/store/` means a Nix build. Expose it through the existing
  `update_status` payload rather than a new command.
- `src/lib/Settings.svelte` / `src/App.svelte` — Nix shows the exact command
  (`nix profile upgrade quota-widget`) as selectable text, reusing the
  `.note code` style already used for `GDK_BACKEND=x11`
  (`styles.css:217-227`); non-Nix Linux shows a release link; Windows shows
  the Windows install button.
- `README.md` — an update matrix beside the existing platform-differences table.

**Owner: Codex.**

### One provider per minor

Each new adapter is its own minor, honouring the one-feature-per-revision rule,
and each takes **the next available minor when it is built**. The order below is
a recommendation, not a queue: these are independent of each other and of the
update chain, so pick whichever Ian wants next. The two plain balance APIs are
listed first only because they are the smallest and share the most code.

| Provider | Endpoint | What it returns |
|---|---|---|
| ~~Firecrawl~~ — **shipped as 0.10.0** | `GET https://api.firecrawl.dev/v2/team/credit-usage` | `remainingCredits` / `planCredits` plus both ends of the billing period. A per-cycle allowance, so it emits a `UsageWindow`; the period bounds are exact rather than inferred |
| ~~DeepSeek~~ — **shipped as 0.11.0** | `GET https://api.deepseek.com/user/balance` | `balance_infos[]` with `total_balance`, `granted_balance`, `topped_up_balance`, and currency (CNY or USD). Amounts arrive as JSON strings; USD is preferred when several currencies are reported |
| ~~Moonshot / Kimi~~ — **shipped as 0.12.0** | `GET https://api.moonshot.ai/v1/users/me/balance` | `available_balance`, `cash_balance`, `voucher_balance`. Keys are platform-specific — a `platform.kimi.ai` key 401s against `.com`, so the base URL is an overridable setting with a **Balance URL** field in Settings |
| ~~Fireworks~~ — **shipped as 0.13.0** | `GET /v1/accounts/{account_id}/billingUsage` | Serverless + dedicated + training costs in nano-USD for the month to date. Needs an account id alongside the key |
| ~~Anthropic Admin~~ — **shipped as 0.14.0** | `GET /v1/organizations/cost_report` | Daily cost buckets. Needs an `sk-ant-admin-*` key, not a normal API key, and the Admin API is unavailable on individual accounts. **`amount` is in cents as a decimal string** — reading it as dollars overstates spend 100× |
| ~~OpenAI Admin~~ — **shipped as 0.14.0** | `GET /v1/organization/costs` | Daily cost buckets. Needs an admin key. `amount` nests `{value, currency}` and is in *dollars* — the opposite of Anthropic's on both counts, so the two do not share a parser |

**Spend-over-period framing — settled by Fireworks in 0.13.0.** Anthropic,
OpenAI, and Fireworks report spend rather than a remaining balance, and this
document left the presentation open pending the first of the three. Fireworks
resolved it (confirmed with Ian): an optional per-account `monthly_budget`
setting turns spend into a `UsageWindow` over the calendar month, which is the
only shape the tray, thresholds, and period marks can act on; with no budget
set, the adapter reports the cost figure alone as `Credits` — labelled "Cost
this month", so it cannot be misread as a balance — and stays out of the
percentage machinery. Both admin adapters follow it. The shape now lives once in
`crates/quota-core/src/providers/spend.rs`, which all three spend providers call;
a fourth should use it rather than reimplementing the branch.

**`Credits.label`** (from Codex's branch) is what makes the no-budget path
readable: an optional name on a monetary figure, so a spend provider renders
"Cost this month: 8.75 USD" while a real balance stays a bare number. It is the
reason a budget is optional rather than required.

**Shared plumbing.** `crates/quota-core/src/providers/simple_credits.rs` landed
with DeepSeek and now backs the balance-style providers. It shares the plumbing
— read key, allow endpoint override, bearer GET, map 401/403 to `AuthExpired`,
decode — not the parsing: each provider supplies a `CreditsSpec` and a `parse`
fn. Allowance-style providers emit a `UsageWindow` and deliberately stay out of
it. The admin adapters need date-range query parameters, so they do not fit it
either.

**Precedent from the shipped ElevenLabs adapter:** when a provider reports an
allowance rather than a balance, emit a `UsageWindow` and leave `credits` as
`None`. `Credits` is for a balance you draw down; a per-cycle allowance belongs
in the same shape as Claude's weekly cap, which is what the tray, thresholds,
and metric pickers already understand. See
`crates/quota-core/src/providers/elevenlabs.rs`.

Per-provider work, the same shape every time:

- `crates/quota-core/src/providers/simple_credits.rs` (new, with whichever of
  DeepSeek/Moonshot lands first) — the
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
  `scripts/smoke-mount.mjs`, `README.md`, `AGENTS.md`, `Cargo.toml`, and (from
  the scroll-to-fade work) `src/lib/opacity.js`, which is imported by both
  windows. Codex authored it; treat it as Codex-owned for later edits.

**Provider adapters are now split by provider, not by file.** The rule above
cannot divide the provider work, because *every* adapter is core crate plus
`Settings.svelte` — assigning them wholesale to Codex leaves nothing to
parallelise. Per Ian, the remaining adapters were divided by what the provider
reports: Claude took the balance/allowance shapes (Firecrawl, DeepSeek,
Moonshot, Fireworks — 0.10.0–0.13.0), Codex the two admin cost-report APIs.
Claude therefore edited `crates/quota-core/**` and `src/lib/Settings.svelte` for
those minors. Outside the provider adapters the file ownership above still
stands.

**Both agents built the same three adapters in parallel** (DeepSeek, Moonshot,
Fireworks) before the split was communicated. Resolved per Ian: Claude's
overlapping three were kept, Codex's two admin adapters were merged in as
0.14.0, and Codex's `Credits.label` was adopted. Two defects were fixed on the
way in — Codex's Fireworks parser looked for top-level `total_cost`/`cost`
fields the API does not return, and its Anthropic parser summed a
cents-denominated `amount` as dollars. **The lesson worth keeping: assign
providers explicitly and confirm before either agent starts**, since a
duplicated adapter costs more to reconcile than to write.

All provider adapters are now shipped. What remains in this plan is the
three-step update chain, which is no longer gated: Ian completed the dist repo
and signing/token secrets on 2026-08-05, so update detection is ready to start.

The patch series is strictly sequential, so ownership there is just *who does
the work*. Real parallelism is available across features: Codex can take the
admin cost adapters while Claude works the update chain. Scroll-to-fade was the
exception — split across both owners on a shared new file, so it was sequenced
rather than parallel (see its Owner note).

When two features are in flight at once, whoever lands first takes the next
minor and the other rebases onto it. Do not reserve a number in advance.

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

Manual checks that cannot be automated, per feature:

- **Corner radius** — rounded corners on Plasma *and* Windows 11, in both light
  and dark themes; drag and resize still work on `main`.
- **Mini height** — no dead space at the bottom for 1, 4, and 8 accounts; the
  summary does not jump when it resizes.
- **Branch badge** — a dispatched branch build shows the badge; a `main` build
  does not.
- **Tailscale transport** — both transports fetch Hermes against a real tailnet
  host.
- **Scroll-to-fade** — scrolling fades both windows against a real desktop
  background on Plasma *and* Windows 11 (a compositor must be running for
  genuine transparency); the popup never becomes an invisible click-trap;
  scrolling the card list and Settings still scrolls rather than fading.
- **New providers** — a real key returns a real figure, and a deliberately wrong
  key surfaces as `AuthExpired` rather than a silent zero.
- **Update chain** — an older build detects a newer `latest.json`; the installer
  path completes and relaunches; a branch build stays silent.

After implementation and the applicable checks are complete, push the feature
branch for review. Do not push `main` or dispatch a Windows build without Ian
saying so.
