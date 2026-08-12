# Quota Widget

A system-tray widget for **Windows 11 and Linux** that watches your AI provider
allowances in one place: Claude's rolling 5-hour window and weekly cap, Codex's
weekly allowance, Hermes Portal credits, OpenRouter credits, ElevenLabs
credits, Firecrawl credits, DeepSeek, Moonshot and Venice balances, and Fireworks, Anthropic and
OpenAI organization spend. It collapses to
the tray and pops up as a compact always-on-top window.

Built with Tauri 2 (Rust) + Svelte 5. The portable EXE is self-contained —
Windows 11 ships the WebView2 runtime it renders with. Linux releases ship an
x86_64 AppImage built on Ubuntu 22.04; a Nix flake remains available to source
repository collaborators (see [Building](#nixos--nix)).

Platform differences are small but real:

| | Windows 11 | Linux |
|---|---|---|
| Tray left-click | Toggles the mini summary | Toggles the mini summary |
| Tray hover | Native detailed tooltip | Plasma-drawn native SNI tooltip |
| Secret storage | Credential Manager (DPAPI) | `0600` plaintext file in the config dir |
| Autostart | `HKCU` run entry | XDG autostart entry |
| Applications-menu entry | Written by the installer | Owned by the Nix package; opt-in and app-managed for the AppImage |
| In-app update | Installed bundle only; installer relaunches the app | AppImage only; offers Restart now / Later |

## How it works

- A tray icon shows the selected mini-summary values' worst-case status at a
  glance: 🟢 ok, 🟡 past your warn threshold, 🔴 past critical, ⚪ grey when
  data is stale or auth failed. Each enabled account can exclude its selected
  value from that calculation in Settings while remaining visible in the
  popup.
- **Left-click** the tray icon to toggle a compact mini summary near the tray.
  It hides when it loses focus unless you pin it with its circle button; the
  pin lasts only for the current app session. Pinning never moves the summary —
  it stays exactly where it is and simply stops being dismissed — and it adopts
  the nearest corner of the screen it's on, so growing to fit new content keeps
  it against that edge. A Settings checkbox controls whether the mini summary
  includes usage bars.
- **Drag the mini summary** by its title bar to move it, including onto another
  screen — unpin it first, since pinning holds its position as well as keeping
  it on top. It snaps to the nearest corner of the screen you drop it on and
  reopens there from then on, whichever screen's tray you click. With more than
  one monitor connected, Settings also offers a picker for the screen alone —
  the corner stays as you dragged it. The chosen screen is remembered by name
  and kept even while it's disconnected, so undocking a laptop shows the
  summary on your primary screen for now and puts it back when you redock.
- **Hover** the tray icon for one native multiline tooltip that lists every
  provider's reported quota windows and balances. Windows draws its standard
  tray tooltip; Plasma draws the StatusNotifierItem tooltip.
- **Right-click** for Open / Refresh now / Settings / Quit. **Open** shows the
  full usage/settings window; reopening it always lands on the usage list.
  **Settings** puts you back where you were when you *leave* it — via Save &
  close or Esc. Opened from the usage list you return to the usage list; opened
  from the tray while the mini summary was up, the summary comes back (pinned
  or not); opened from the tray with nothing on screen, you end with nothing on
  screen. The ← back arrow is different: it is navigation inside the window, so
  it always shows the usage list and leaves the window open, whatever you came
  from. Back and Esc discard unsaved edits — Save & close is the only thing
  that writes — and a save that fails leaves Settings open with its error. ✕
  always means *hide the widget*, whatever you came from.
- **Scroll** over either window to fade it: up towards transparent, down back
  to opaque. The two windows keep that level for different lengths of time. The
  **full popup** forgets it on every open, so it always comes back opaque. The
  **mini summary** keeps exactly the level you left it at — through tray
  toggles, clicking away, its close button, and while the full window is open —
  for as long as the app is running; restarting the app opens it opaque again.
  The level is never written to your configuration either way. Scrolling the
  card list or the Settings form still scrolls normally, and every window
  stops at the same faint-but-visible 15% floor rather than becoming an
  invisible thing that eats clicks. A mini summary left at that floor comes
  back just as faint on the next tray click rather than fully opaque. Unticking
  the Settings checkbox turns the behaviour off and restores both windows to
  fully opaque; ticking it again starts from opaque rather than from the level
  you had before.
- **Account order** is yours by default: accounts appear in the order you
  arranged them in Settings. Two Settings dropdowns can instead sort them by
  usage (high→low or low→high) or by expiry (soonest or furthest reset), with a
  second dropdown choosing *which* number sorts — the value the account
  contributes to the tray icon, or its worst window regardless of which
  headlines you selected. That choice also decides which window's reset time
  the expiry orders use. Sorting applies to the main window, the mini summary,
  and the tray tooltip alike, so the three never disagree. Accounts with no
  such number — a credits-only balance, one pinned to "None", or one whose
  fetch just failed — sink to the bottom in your own order rather than being
  ranked on a number they don't have.
- **Launching starts in the tray**, whether you run it yourself or it starts
  with your session — no window appears until you ask for one, from the tray
  icon or by running the app a second time (which opens and focuses the full
  window). If a tray icon cannot be created at all, the full window opens
  instead, so the app is never running with no way to reach it.
- A background poller (default every 60 s) refreshes all enabled providers and
  fires alerts when usage *crosses* a threshold (edge-triggered — you get one
  toast at 80%, not one per poll). Toast, tray color, and auto-popup are each
  independently toggleable, globally and per provider.
- The **first successful poll of each account is a baseline**, not a crossing.
  Being already over a threshold when the widget starts is a state, not
  something that just happened, so it colours the tray icon and appears in the
  tooltip without opening a window — auto-popup included. A baseline *critical*
  state still sends a toast (if toasts are on); a baseline warning stays
  tray-only. Everything after that baseline behaves exactly as before, and an
  account whose first fetch fails takes its baseline from the first one that
  works.
- Where a provider reports exact allowance figures or you set a monthly budget,
  the main window shows remaining and total below the percentage meter. The
  compact summary and tray keep their percentage-only form for quick scanning.

## Provider setup

| Provider | What you need | Notes |
|---|---|---|
| **Claude** | A Claude Pro/Max login — either the Claude Code CLI (`claude`) or the widget's built-in browser sign-in | Calls the same usage endpoint Claude Code's `/usage` uses; shows the 5-hour window, weekly cap, and any per-model weekly caps the API reports. **Sign-in method** in Settings: *Auto* (default) prefers a fresh CLI token from `%USERPROFILE%\.claude\.credentials.json`, else the widget's own login; *Built-in* runs a PKCE browser sign-in (click "Sign in with Claude", authorize, paste back the code) — ideal if you only use Claude Desktop. When the widget refreshes a token itself, the rotated pair is stored in its own secret store and never written to Claude Code's file. Unofficial endpoints — may change. |
| **Codex** | A ChatGPT plan — either the Codex CLI (`codex`) or the widget's built-in device sign-in | Calls the ChatGPT backend usage endpoint the Codex CLI's `/status` uses; renders whatever rate-limit windows the response contains (weekly today; adapts automatically if OpenAI reshapes it). **Sign-in method** in Settings: *Auto* (default) prefers `%USERPROFILE%\.codex\auth.json`, else the widget's own login; *Built-in* runs the same device flow as `codex login --device-auth` — click "Sign in with Codex", then type the short code shown into the browser page that opens. Note this flow is proprietary rather than RFC 8628, and undocumented: it's reimplemented from the Codex CLI source and can change without notice. Some accounts need an admin to enable device sign-in. Unofficial endpoints. |
| **OpenRouter** | An API key from [openrouter.ai/keys](https://openrouter.ai/keys) | Official `GET /api/v1/credits` API. Shows balance and lifetime usage in USD. Set an optional **Monthly budget** to track spend against a target; the widget records a month-start baseline because the API does not report month-to-date usage. |
| **ElevenLabs** | An API key from [elevenlabs.io/app/settings/api-keys](https://elevenlabs.io/app/settings/api-keys) | Official `GET /v1/user/subscription` API. Shows the billing cycle's credit allowance as a usage window — used vs. limit, labelled “Credits”, counting down to the cycle reset. On plans with credit-limit extension enabled, usage can read past 100%. |
| **Firecrawl** | An API key from [firecrawl.dev/app/api-keys](https://firecrawl.dev/app/api-keys) | Official `GET /v2/team/credit-usage` API. Shows the billing cycle's plan credits as a usage window — spent vs. granted, labelled “Credits” — plus the exact remaining/plan figures in the main window. Bonus credits above the plan correctly read as negative usage. The response carries both ends of the billing period, so the period-progress marker is exact rather than inferred. |
| **DeepSeek** | An API key from [platform.deepseek.com/api_keys](https://platform.deepseek.com/api_keys) | Official `GET /user/balance` API. Shows the remaining balance in the account's own currency (CNY or USD); if an account reports both, USD is displayed. DeepSeek reports only what remains, never what was spent, so the card shows a balance without a usage percentage. |
| **Moonshot / Kimi** | An API key from [platform.kimi.ai](https://platform.kimi.ai) | Official `GET /v1/users/me/balance` API. Shows `available_balance` in USD — the figure that actually gates calls, since Moonshot rejects requests with `exceeded_current_quota_error` once it hits zero. **Keys are platform-specific:** `platform.kimi.ai` and `platform.kimi.com` issue independent keys and a key from one returns 401 against the other, so a `.com` key needs **Balance URL** in Settings pointed at that host. |
| **Venice** | An API key from [venice.ai](https://venice.ai) | Official `GET /api/v1/api_keys/rate_limits`. Reports **two** balances — USD and DIEM — so **Headline balance** in Settings picks which one heads the card; the other is shown as an informational row. The same response carries `accessPermitted`, so a key that is valid but suspended is surfaced rather than looking healthy, and `nextEpochBegins` gives an exact reset instant. A 402 (valid key, no funds) renders as a zero balance rather than a fetch error. |
| **OneHop** | An API key from the OneHop console | `GET /v1/user/balance` — the gateway's prepaid wallet balance in USD, so it renders as a plain balance with no percentage. **Undocumented:** OneHop's published docs describe the wallet but specify no endpoint for it, so this may change without notice. It is still the right route — the console's own billing page authenticates with a full-access session cookie, while this answers to a scoped API key. |
| **Fireworks** | An API key from [fireworks.ai/account/api-keys](https://fireworks.ai/account/api-keys), plus your **account ID** | Official `GET /v1/accounts/{account_id}/billingUsage` API, summing serverless, dedicated and training costs for the calendar month to date. Fireworks reports *spend*, not a balance or an allowance, so there is no percentage unless you say what a full month looks like: set an optional **Monthly budget** and the card shows spend against it as a usage window (with tray colour, thresholds and a period marker); leave it blank and the card shows month-to-date spend as a plain figure. Overspending a budget reads past 100% — it's your intention, not a cap Fireworks enforces. |
| **Anthropic Admin** | An **admin** key (`sk-ant-admin…`) from Console → Settings → Admin keys | Official `GET /v1/organizations/cost_report` API, summing the daily cost buckets for the calendar month to date. Needs an admin key, not a normal API key, and the Admin API is unavailable on individual accounts. Like Fireworks it reports *spend*: set a **Monthly budget** to see it as a usage window, or leave it blank for a plain "Cost this month" figure. Note Priority Tier spend is excluded by the API itself. |
| **OpenAI Admin** | An organization **Admin** key from [platform.openai.com/settings/organization/admin-keys](https://platform.openai.com/settings/organization/admin-keys) | Official `GET /v1/organization/costs` API, summing the daily cost buckets for the calendar month to date. Needs an admin key, not a normal API key. Same spend framing and **Monthly budget** option as the other spend providers. |
| **Hermes Portal** | hermes-agent installed and logged in (`hermes`) — zero extra setup | Reads the Nous OAuth access token from `~/.hermes/auth.json` and calls the portal's billing API (`/api/billing/state` + `/api/billing/subscription`): purchased-credit balance in USD, monthly subscription allowance with tier name and cycle-reset countdown, and monthly-cap usage where configured. Subscription rollover appears as a negative usage percentage alongside its exact remaining/plan credits; the monthly cap remains a percentage only because it is a spending ceiling, not an allowance. Set an optional **Monthly budget** to add a monthly-spend target without hiding the purchased balance; Hermes' own `spentThisMonthUsd` is used when available, otherwise the widget tracks drawdown from the month's opening balance and handles top-ups. The subscription allowance is shown greyed-out and does **not** colour the card or tray while a purchased balance is still funding calls — on the Free tier that allowance is a fraction of a credit and reads 100% used permanently, which is not a quota you're actually hitting. The widget only ever uses the short-lived *access* token — never hermes's refresh token, which the portal rotates and revokes on reuse — so a stale token means "run any `hermes` command" (or keep hermes running; its keepalive refreshes it). **No hermes on this machine?** Set Settings → Hermes → Source to *Remote hermes over SSH* and enter `user@server`: the widget fetches the auth file from a machine that does run hermes (`ssh <host> cat .hermes/auth.json`, BatchMode — needs working key auth; Windows 10/11 include the OpenSSH client). Set **Transport** to *Tailscale SSH* instead when the remote is on your tailnet; the widget then runs `tailscale ssh <host>` and passes the same non-interactive SSH options through to OpenSSH. Last resort: paste a portal session cookie. |

You can add multiple named accounts of each provider in Settings. The editable
account name is shown everywhere; its internal key stays fixed so changing a
name never loses its stored sign-in. New accounts copy an existing account's
provider settings, while their API keys and OAuth sign-ins remain separate.
Every account, including the original defaults, can be removed.
Accounts can be moved up and down in Settings; that order is saved and is the
order used in the full popup, mini summary, and tray tooltip. Each account can
also choose its own mini-summary headline: Automatic picks the worst real quota
window (or credits), while a specific window or credit balance keeps that
compact row focused. Choose None to omit the account from the compact summary.
The selected value can also contribute to the tray icon's status and gauge;
this does not change alerts or card status.

Only the Settings fields scroll: **Save & close** sits in a fixed footer at the
bottom of the window, with the app version beneath it, so the commit action is
reachable from anywhere in the form. Save is the only thing that writes — if it
fails, Settings stays open and shows the error in that footer rather than
closing on a write that never landed.

Secrets (API keys, cookies, OAuth tokens) are stored in the **Windows Credential
Manager**, not on disk. On Linux they fall back to a `0600` `secrets.json` in the
config dir. Config lives at `%APPDATA%\quota-widget\config.json`
(`~/.config/quota-widget/config.json` on Linux).

**If that config file cannot be read**, the widget starts on defaults and says
so in a banner across the top of the popup, naming the file. Your file is left
exactly as it is, and every save — from Settings, from moving the mini summary,
from anywhere — refuses while it is in that state, so nothing overwrites it
behind your back. You then have two ways out: fix or restore the file by hand
and reopen the widget, or press **Replace with these settings** in the banner,
which moves your unreadable file to `config.json.unreadable` in the same folder
(nothing is deleted) and starts saving normally again. A first run, where there
is no config file at all, is not this case: it simply starts with the defaults.

**What that protects against, and what it doesn't.** The two platforms are not
equivalent, so it is worth being precise:

- **Windows.** Credential Manager encrypts entries with DPAPI, tied to your
  Windows account. Another user on the same machine cannot read them, and they
  are not recoverable from a stolen disk or a file-level backup. They *are*
  readable by anything running as you — DPAPI unwraps automatically for your
  own session — so this is protection against other accounts and offline
  access, not against malware you are running.
- **Linux.** `secrets.json` is **plaintext**, protected only by file
  permissions. Root, any process running as you, and any backup that includes
  your home directory can read it. Roughly the same exposure as a `.env` file,
  with two small advantages: it is `0600` rather than the usual `644`, and it
  lives in `~/.config` rather than a project directory where `git add -A` or a
  Docker build context might sweep it up. The mode is set by the `open` call
  that creates the file, so no byte of a key is ever on disk world-readable,
  and on a filesystem that ignores POSIX permissions (an exFAT mount, some
  network shares) the save is **refused with an error** rather than quietly
  leaving your keys readable. Updates are written to a temp file in the same
  directory and renamed over the store, so an interrupted save leaves either
  the old store or the new one, never half a file.

`keyring` supports the Secret Service API, so GNOME Keyring / KWallet storage
on Linux is a small change if that exposure ever stops being acceptable. It has
not been done because the Linux target so far is a single-user desktop.

Judge accordingly what you paste in. A read-only usage key is a very different
prospect from an organization admin key, and the admin providers here
deliberately want the latter.

### How far each provider has been verified

Every adapter is written against the vendor's documented response schema and
covered by unit tests over that schema. What follows is what has additionally
been seen from a **live account** — worth knowing before you trust a number.

- **Verified with real usage data:** Firecrawl. Its parse path has run on a
  meaningful non-zero reading, so the arithmetic and formatting are exercised,
  not just the plumbing.
- **Reached successfully, but only on an unused account reporting $0.00:**
  DeepSeek, Moonshot, OneHop, Fireworks, OpenAI Admin. This confirms the key is
  accepted, the endpoint is right, and the response parses — but a zero says
  nothing about whether a non-zero amount is scaled correctly.
- **Not yet run against a live account:** Anthropic Admin, Venice.

That middle distinction is not pedantry. Anthropic's cost report returns
`amount` in **cents** while OpenAI's returns **dollars**, so a units mistake is
a 100× error that a $0.00 reading cannot possibly reveal (see the units note in
`crates/quota-core/src/providers/anthropic_admin.rs`). Treat the first non-zero
figure from any provider in the lower two groups as worth sanity-checking
against that vendor's own dashboard.

## Building

### CI

Push to GitHub — `.github/workflows/build.yml` runs the core test suite on Linux
for every branch. Windows packaging is **dispatch-only** (`gh workflow run
build.yml --ref <branch>`), because that runner bills at a **2x** minute
multiplier against the account's monthly Actions quota. A dispatch produces two
artifacts:

- `quota-widget-portable` — the single portable `quota-widget.exe`
- `quota-widget-installer` — an NSIS installer, if you'd rather have Start Menu
  integration and an uninstaller

Release tags belong to `.github/workflows/release.yml` instead — see
[Deployment](#deployment). The two workflows deliberately do not both watch
tags: when they did, one tag paid for two full Windows builds and produced two
competing sets of assets.

### NixOS / Nix

The repo is a flake (Linux only; verified build):

```sh
nix run github:harmanhobbit/quota-widget   # or nix build / nix profile install
```

In a device flake, either add the overlay
(`nixpkgs.overlays = [ quota-widget.overlays.default ];` →
`environment.systemPackages = [ pkgs.quota-widget ];`) or take
`quota-widget.packages.${system}.default` directly. The package wraps the
binary with the GTK/WebKit runtime and puts `ssh` and `tailscale` on PATH for
the Hermes remote source. The tray itself uses the native D-Bus
StatusNotifierItem protocol.
Bumping npm deps later means refreshing `npmDeps.hash` in `nix/package.nix`.

### Locally

```sh
npm install
npm run gen-icons          # regenerate src-tauri/icons (already committed)
cargo test -p quota-core   # pure-Rust core: parsers, alert engine, config
npm run check-versions     # guards the single-source version (see below)
npm run tauri dev          # run the app (needs OS webview libs — see below)
npm run tauri build        # produce the exe (run this on Windows)
```

**Versioning.** The workspace `Cargo.toml` is the single source of truth.
`tauri.conf.json` omits its `version` field (Tauri falls back to Cargo.toml),
`nix/package.nix` reads it via `lib.importTOML`, and `package.json` has no
version at all. Bumping a release is a one-line edit to `Cargo.toml` followed by
`cargo update -w`; `npm run check-versions` fails if a hardcoded copy reappears.

- **On Windows**: install Rust + Node, then the two commands above just work —
  except `npm run tauri build`, which now signs its bundle. The config carries
  an updater pubkey, so Tauri refuses to bundle without the matching private
  key and fails with *"A public key has been found, but no private key"*. Set
  `TAURI_SIGNING_PRIVATE_KEY` to the key's path or contents (add
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` if yours has a passphrase). `npm run
  tauri dev` and `cargo build` are unaffected — only bundling signs. If you
  only want the raw EXE and no installer, `cargo build --release -p
  quota-widget` skips the bundle step entirely.
- **On Linux with Nix** (easiest): the flake ships a dev shell with the whole
  toolchain — Rust, Node, `cargo-tauri`, and the GTK/WebKit libraries the
  `-sys` crates need at build time. `nix develop`, or `direnv allow` once and
  the committed `.envrc` enters it whenever you `cd` in. Install
  [nix-direnv](https://github.com/nix-community/nix-direnv) so the shell is
  cached rather than re-evaluated at every prompt.
- **On Linux without Nix** (dev runs): install the Tauri prerequisites first:
  `sudo apt install libwebkit2gtk-4.1-dev build-essential pkg-config libgtk-3-dev librsvg2-dev`
- **Cross-compiling Windows EXEs from Linux** is possible but officially
  experimental: `cargo install cargo-xwin`, add the `x86_64-pc-windows-msvc`
  target, install `nsis` + `lld`, then
  `npm run tauri build -- --runner cargo-xwin --target x86_64-pc-windows-msvc`.
  Prefer the CI route.

## Deployment

Copy `quota-widget.exe` anywhere and run it. First run: right-click the tray
icon → Settings, enable the providers you use, paste any keys, set thresholds,
and optionally enable **Start on login** (a `HKCU` run entry on Windows, an XDG
autostart entry on Linux — no admin rights needed either way). Updating =
replacing the EXE; on Nix, `nix profile upgrade`.

**Linux releases** publish a signed x86_64 AppImage, built on pinned Ubuntu
22.04 as the compatibility floor. Download the `.AppImage` and its `.sig` from
the public dist repository, then run `chmod +x QuotaWidget_<version>_amd64.AppImage`
followed by `./QuotaWidget_<version>_amd64.AppImage`. The public download page
has the matching `minisign` verification command. The Nix flake is distinct: it
is a reproducible source build that pins GTK/WebKit, and remains available only
to collaborators with access to this private repository.

That floor is checked by launching the AppImage directly on a Kubuntu 22.04 VM
with nothing extra installed. Running it on NixOS through `appimage-run` is
supplemental coverage only: `appimage-run` supplies its own glibc and GTK/WebKit
runtime, so it shows the app works under Nix's shims rather than that the binary
fits the userspace a stock Ubuntu 22.04 system provides.

#### AppImage desktop integration

An AppImage is just a file you downloaded, so nothing puts it in your
applications menu. The first time you run one, the widget **asks** whether to
add a launcher, and remembers your answer either way — "Not now" is remembered
as firmly as yes, so it never asks twice. Settings → **Applications menu** has
explicit **Add** and **Remove** buttons afterwards, so a deferral is never
final.

It is deliberately small: a `.desktop` file and two icons under your own
`$XDG_DATA_HOME` (`~/.local/share` by default), nothing system-wide, no
`appimaged` and no background daemon. The launcher runs
`env GDK_BACKEND=x11 <your AppImage>`, matching the Nix package's entry — the
XWayland workaround is what lets the popup position and raise itself (see
[Known limitations](#known-limitations)).

The launcher points at the AppImage **where it is**, not at a copy. Replacing
the file in place — which is what the in-app update does — keeps it working.
*Moving* it does not, so the widget then offers to **repair** the launcher
rather than silently retargeting it.

Removal only deletes files the app wrote and you have not since edited: a
launcher or icon you changed is left exactly as it is, and Settings tells you
where it is so you can delete it yourself. A `.desktop` file at that path that
the app did not write is never touched at all — Settings says so instead of
offering buttons that would overwrite it.

### Releases and update checks

Cut a release with **`npm run release`**, which prompts for the version and
handles the bump, `Cargo.lock`, commit, annotated tag, and push — confirming
before anything leaves the machine. See "Releases" in `AGENTS.md` for what it
does and the by-hand equivalent.

This repo is private, so releases are published to the public
[`harmanhobbit/quota-widget-dist`](https://github.com/harmanhobbit/quota-widget-dist)
repo: pushing a `v*.*.*` tag runs `release.yml`, which builds signed Windows
and Ubuntu-22.04 AppImage artifacts, then uploads the NSIS installer, its
`.sig`, the portable EXE (renamed
`QuotaWidget_<version>_x64-portable.exe`, since an asset name is its download
URL and a bare `quota-widget.exe` reads identically across every release), the
AppImage and its `.sig`, and a `latest.json` manifest. It only publishes after
both platform builds succeed. It also republishes `docs/dist-README.md` as that
repo's landing page, so the public download instructions cannot drift from what
ships — edit that file, not the dist repo directly. The tag must match the
workspace `Cargo.toml` version — CI refuses to publish a mislabelled tree. A `workflow_dispatch` defaults to a **dry run**: it builds and signs, then
attaches the manifest and installer as workflow artifacts instead of publishing,
which is the safe way to exercise the signing key end to end.

Linux needs a manual pass on top of that, because nothing in a build log shows
the tray, window placement against a real panel, the launcher written into a
home directory, or an update that rewrites the running executable.
[`docs/linux-release-validation.md`](docs/linux-release-validation.md) is the
checklist: what to look for in the dry run's combined `latest.json`, then
launch, tray, popup and mini-summary placement, opt-in desktop integration and
one end-to-end signed update on a Kubuntu 22.04 VM — with a reduced
launch/tray/popup set for every later release.

The app checks that manifest at startup and every six hours, and shows an
unobtrusive "Update available" line in Settings with a **Check now** button.
Uncheck **Check for updates** to switch the automatic checks off; **Check now**
keeps working regardless, since pressing it is itself the consent. Builds from a
feature branch never check at all — they carry a dev badge and would otherwise
nag about a "newer" release they are actually ahead of.

Settings offers **Install update** only when the running app is an *installable
artifact* — a bundle `tauri-plugin-updater` can replace. That is a separate
fact from whether the release published a download: a portable EXE finds the
Windows installer in the manifest and still cannot replace itself. Either way
the download is verified against the minisign `pubkey` in `tauri.conf.json`
before anything is run or written.

Manifest keys reflect this. Windows publishes one installer, so its entry is
the bare `windows-x86_64`; Linux has several mutually exclusive package
formats, so the AppImage is published under the artifact-qualified
`linux-x86_64-appimage`. A future `.deb` or Flatpak gets its own key rather
than colliding with the AppImage's, and each build only ever selects the entry
matching the format it is actually running as.

The two platforms then finish differently, and the UI says which:

- **Windows** downloads the new NSIS installer and runs it in `passive` mode.
  The app exits partway through and the installer brings it back, so the UI
  warns that it is about to close and reopen.
- **Linux** replaces the running AppImage in place. The old process keeps
  running the old code — nothing relaunches on its own — so Settings offers
  **Restart now** and **Later** once the install finishes, and never claims an
  automatic relaunch. *Later* is not a deferral of the install: the new version
  is already on disk and starts at the next launch.

Everything else — a portable EXE, a Nix or other package-managed install, or a
platform the release published nothing for — gets the "upgrade the way you
installed it" guidance rather than an action that would fail.

`updater:default` is granted in `capabilities/default.json` only. It is
deliberately **not** in `mini.json`: the tray-click summary has no business
holding install rights.

## Architecture

```
crates/quota-core   pure Rust, no UI deps — fully unit-tested
  model.rs          UsageSnapshot / UsageWindow / Credits / FetchError
  config.rs         config persistence + per-provider overrides
  alerts.rs         edge-triggered alert engine
  desktop.rs        per-user AppImage launcher/icon integration (plan + apply)
  settings_return.rs  where a Settings visit goes when it exits
  secret_store.rs   secret key naming + the owner-only, atomic plaintext store
  providers/        one adapter per provider behind a common trait
src-tauri           the Tauri shell
  tray.rs           runtime-generated status icons, full window + mini summary placement
  poller.rs         poll loop → state → tray/toasts/events
  oauth.rs          built-in Claude sign-in (PKCE paste-back)
  codex_oauth.rs    built-in Codex sign-in (device code)
  updates.rs        6-hourly check of the public release manifest
  desktop.rs        IPC around quota-core's AppImage desktop integration
  secrets.rs        Credential Manager (Windows) / quota-core's file store (elsewhere)
src/                Svelte UI
  App.svelte        popup shell (usage list / settings)
  lib/              ProviderCard, Settings, MiniSummary
scripts/            icon generation, version-drift guard
```

## Known limitations

- Claude and Codex usage endpoints are the CLIs' private APIs; a provider-side
  change can break those cards until this widget is updated. The cards degrade
  to a labelled error state rather than crashing. The same applies doubly to
  Codex's built-in sign-in: its device flow is proprietary (not RFC 8628) and
  undocumented, so the endpoints and client id are pinned to what the Codex CLI
  source did when this was written. If it breaks, `codex login` still works and
  Auto mode picks that up.
- The Hermes adapter scans responses leniently for balance-like fields, but the
  portal may change shape; the endpoint URL is user-configurable for that
  reason.
- Most balance and spend adapters have only been confirmed against accounts
  reporting **$0.00**, and the Anthropic Admin one has not been run against a
  live account at all — so a first non-zero reading is worth checking against
  the vendor's dashboard. See
  [How far each provider has been verified](#how-far-each-provider-has-been-verified).
- The Claude "weekly" window's reset cadence is whatever the API reports —
  observed in the wild resetting more often than every 7 days.
- Tray hover is always a native tooltip, not a widget window: Windows renders
  the standard tray tooltip and Linux Plasma renders the StatusNotifierItem
  tooltip. The Linux launcher uses XWayland so pinned mini-summary placement
  and always-on-top work.
- **Always-on-top does not work on native Wayland**, so the popup slips behind
  other windows when they take focus — regardless of the *Hide when clicking
  outside* setting, which is a separate mechanism. This is a protocol gap, not
  a widget bug: `xdg-shell` has no `set_always_on_top`, GTK dropped its old
  one, and no portable replacement exists (GNOME and wlroots each expose their
  own). Tracked upstream as [tao#1134](https://github.com/tauri-apps/tao/issues/1134)
  and [tauri#3117](https://github.com/tauri-apps/tauri/issues/3117), both
  labelled *upstream*. Running under XWayland restores it:

  ```sh
  GDK_BACKEND=x11 quota-widget
  ```

  Worth knowing before you switch: XWayland can look blurry under fractional
  scaling. X11 and Windows are unaffected.
