# Quota Widget

A system-tray widget for **Windows 11 and Linux** that watches your AI provider
allowances in one place: Claude's rolling 5-hour window and weekly cap, Codex's
weekly allowance, Hermes Portal credits, OpenRouter credits, ElevenLabs
credits, Firecrawl credits, and DeepSeek and Moonshot balances. It collapses to
the tray and pops up as a compact always-on-top window.

Built with Tauri 2 (Rust) + Svelte 5. The portable EXE is self-contained —
Windows 11 ships the WebView2 runtime it renders with. On Linux it's packaged as
a Nix flake (see [Building](#nixos--nix)).

Platform differences are small but real:

| | Windows 11 | Linux |
|---|---|---|
| Tray left-click | Toggles the mini summary | Toggles the mini summary |
| Tray hover | Native detailed tooltip | Plasma-drawn native SNI tooltip |
| Secret storage | Credential Manager | `0600` file in the config dir |
| Autostart | `HKCU` run entry | XDG autostart entry |

## How it works

- A tray icon shows the selected mini-summary values' worst-case status at a
  glance: 🟢 ok, 🟡 past your warn threshold, 🔴 past critical, ⚪ grey when
  data is stale or auth failed. Each enabled account can exclude its selected
  value from that calculation in Settings while remaining visible in the
  popup.
- **Left-click** the tray icon to toggle a compact mini summary near the tray.
  It hides when it loses focus unless you pin it with its circle button; the
  pin lasts only for the current app session. A Settings checkbox controls
  whether the mini summary includes usage bars.
- **Hover** the tray icon for one native multiline tooltip that lists every
  provider's reported quota windows and balances. Windows draws its standard
  tray tooltip; Plasma draws the StatusNotifierItem tooltip.
- **Right-click** for Open / Refresh now / Settings / Quit. **Open** shows the
  full usage/settings window; reopening it always lands on the usage list.
- **Scroll** over either window to fade it: up towards transparent, down back
  to opaque. The level is temporary and resets whenever the window is shown
  again, so a faded widget is never left that way. Scrolling the card list or
  the Settings form still scrolls normally, and the full popup stops at a
  faint-but-visible floor rather than becoming an invisible thing that eats
  clicks — as does a *pinned* mini summary, which ignores click-away. An
  unpinned summary may fade all the way out, since clicking elsewhere
  dismisses it. A Settings checkbox turns the whole behaviour off.
- A background poller (default every 60 s) refreshes all enabled providers and
  fires alerts when usage *crosses* a threshold (edge-triggered — you get one
  toast at 80%, not one per poll). Toast, tray color, and auto-popup are each
  independently toggleable, globally and per provider.

## Provider setup

| Provider | What you need | Notes |
|---|---|---|
| **Claude** | A Claude Pro/Max login — either the Claude Code CLI (`claude`) or the widget's built-in browser sign-in | Calls the same usage endpoint Claude Code's `/usage` uses; shows the 5-hour window, weekly cap, and any per-model weekly caps the API reports. **Sign-in method** in Settings: *Auto* (default) prefers a fresh CLI token from `%USERPROFILE%\.claude\.credentials.json`, else the widget's own login; *Built-in* runs a PKCE browser sign-in (click "Sign in with Claude", authorize, paste back the code) — ideal if you only use Claude Desktop. When the widget refreshes a token itself, the rotated pair is stored in its own secret store and never written to Claude Code's file. Unofficial endpoints — may change. |
| **Codex** | A ChatGPT plan — either the Codex CLI (`codex`) or the widget's built-in device sign-in | Calls the ChatGPT backend usage endpoint the Codex CLI's `/status` uses; renders whatever rate-limit windows the response contains (weekly today; adapts automatically if OpenAI reshapes it). **Sign-in method** in Settings: *Auto* (default) prefers `%USERPROFILE%\.codex\auth.json`, else the widget's own login; *Built-in* runs the same device flow as `codex login --device-auth` — click "Sign in with Codex", then type the short code shown into the browser page that opens. Note this flow is proprietary rather than RFC 8628, and undocumented: it's reimplemented from the Codex CLI source and can change without notice. Some accounts need an admin to enable device sign-in. Unofficial endpoints. |
| **OpenRouter** | An API key from [openrouter.ai/keys](https://openrouter.ai/keys) | Official `GET /api/v1/credits` API. Shows balance and usage in USD. |
| **ElevenLabs** | An API key from [elevenlabs.io/app/settings/api-keys](https://elevenlabs.io/app/settings/api-keys) | Official `GET /v1/user/subscription` API. Shows the billing cycle's credit allowance as a usage window — used vs. limit, labelled “Credits”, counting down to the cycle reset. On plans with credit-limit extension enabled, usage can read past 100%. |
| **Firecrawl** | An API key from [firecrawl.dev/app/api-keys](https://firecrawl.dev/app/api-keys) | Official `GET /v2/team/credit-usage` API. Shows the billing cycle's plan credits as a usage window — spent vs. granted, labelled “Credits”. The response carries both ends of the billing period, so the period-progress marker is exact rather than inferred. |
| **DeepSeek** | An API key from [platform.deepseek.com/api_keys](https://platform.deepseek.com/api_keys) | Official `GET /user/balance` API. Shows the remaining balance in the account's own currency (CNY or USD); if an account reports both, USD is displayed. DeepSeek reports only what remains, never what was spent, so the card shows a balance without a usage percentage. |
| **Moonshot / Kimi** | An API key from [platform.kimi.ai](https://platform.kimi.ai) | Official `GET /v1/users/me/balance` API. Shows `available_balance` in USD — the figure that actually gates calls, since Moonshot rejects requests with `exceeded_current_quota_error` once it hits zero. **Keys are platform-specific:** `platform.kimi.ai` and `platform.kimi.com` issue independent keys and a key from one returns 401 against the other, so a `.com` key needs **Balance URL** in Settings pointed at that host. |
| **Hermes Portal** | hermes-agent installed and logged in (`hermes`) — zero extra setup | Reads the Nous OAuth access token from `~/.hermes/auth.json` and calls the portal's billing API (`/api/billing/state` + `/api/billing/subscription`): purchased-credit balance in USD, monthly subscription allowance with tier name and cycle-reset countdown, and monthly-cap usage where configured. The subscription allowance is shown greyed-out and does **not** colour the card or tray while a purchased balance is still funding calls — on the Free tier that allowance is a fraction of a credit and reads 100% used permanently, which is not a quota you're actually hitting. The widget only ever uses the short-lived *access* token — never hermes's refresh token, which the portal rotates and revokes on reuse — so a stale token means "run any `hermes` command" (or keep hermes running; its keepalive refreshes it). **No hermes on this machine?** Set Settings → Hermes → Source to *Remote hermes over SSH* and enter `user@server`: the widget fetches the auth file from a machine that does run hermes (`ssh <host> cat .hermes/auth.json`, BatchMode — needs working key auth; Windows 10/11 include the OpenSSH client). Set **Transport** to *Tailscale SSH* instead when the remote is on your tailnet; the widget then runs `tailscale ssh <host>` and passes the same non-interactive SSH options through to OpenSSH. Last resort: paste a portal session cookie. |

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

Secrets (API keys, cookies, OAuth tokens) are stored in the **Windows Credential
Manager**, not on disk. On Linux they fall back to a `0600` `secrets.json` in the
config dir. Config lives at `%APPDATA%\quota-widget\config.json`
(`~/.config/quota-widget/config.json` on Linux).

## Building

### CI

Push to GitHub — `.github/workflows/build.yml` runs the core test suite on Linux
and produces two artifacts on a Windows runner. Note the Windows runner bills at
a **2x** minute multiplier against the account's monthly Actions quota, so it's
worth doing routine work in the local dev shell and saving CI for the EXE you
actually intend to test:

- `quota-widget-portable` — the single portable `quota-widget.exe`
- `quota-widget-installer` — an NSIS installer, if you'd rather have Start Menu
  integration and an uninstaller

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

- **On Windows**: install Rust + Node, then the two commands above just work.
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

## Architecture

```
crates/quota-core   pure Rust, no UI deps — fully unit-tested
  model.rs          UsageSnapshot / UsageWindow / Credits / FetchError
  config.rs         config persistence + per-provider overrides
  alerts.rs         edge-triggered alert engine
  providers/        one adapter per provider behind a common trait
src-tauri           the Tauri shell
  tray.rs           runtime-generated status icons, full window + mini summary placement
  poller.rs         poll loop → state → tray/toasts/events
  oauth.rs          built-in Claude sign-in (PKCE paste-back)
  codex_oauth.rs    built-in Codex sign-in (device code)
  secrets.rs        Credential Manager (Windows) / 0600 file (elsewhere)
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
