# Quota Widget

A Windows 11 system-tray widget that watches your AI provider allowances in one
place: Claude's rolling 5-hour window and weekly cap, Codex's weekly allowance,
Hermes Portal credits, and OpenRouter credits. It collapses to the tray and pops
up as a compact always-on-top window.

Built with Tauri 2 (Rust) + Svelte 5. The portable EXE is self-contained —
Windows 11 ships the WebView2 runtime it renders with.

## How it works

- A tray icon shows worst-case status at a glance: 🟢 ok, 🟡 past your warn
  threshold, 🔴 past critical, ⚪ grey when data is stale or auth failed. Each
  provider has an **Include in tray icon** checkbox in Settings — clear it to
  keep a provider on the popup without letting it drive the tray.
- **Left-click** the tray icon to toggle the popup near the tray. **Esc**,
  clicking elsewhere, or the ✕ button hides it again — the app keeps running.
- **Right-click** for Open / Refresh now / Settings / Quit.
- A background poller (default every 60 s) refreshes all enabled providers and
  fires alerts when usage *crosses* a threshold (edge-triggered — you get one
  toast at 80%, not one per poll). Toast, tray color, and auto-popup are each
  independently toggleable, globally and per provider.

## Provider setup

| Provider | What you need | Notes |
|---|---|---|
| **Claude** | A Claude Pro/Max login — either the Claude Code CLI (`claude`) or the widget's built-in browser sign-in | Calls the same usage endpoint Claude Code's `/usage` uses; shows the 5-hour window, weekly cap, and any per-model weekly caps the API reports. **Sign-in method** in Settings: *Auto* (default) prefers a fresh CLI token from `%USERPROFILE%\.claude\.credentials.json`, else the widget's own login; *Built-in* runs a PKCE browser sign-in (click "Sign in with Claude", authorize, paste back the code) — ideal if you only use Claude Desktop. When the widget refreshes a token itself, the rotated pair is stored in its own secret store and never written to Claude Code's file. Unofficial endpoints — may change. |
| **Codex** | Codex CLI installed and logged in (`codex`) | Reads `%USERPROFILE%\.codex\auth.json` and calls the ChatGPT backend usage endpoint. Renders whatever rate-limit windows the response contains (weekly today; adapts automatically if OpenAI reshapes it). Unofficial endpoint. |
| **OpenRouter** | An API key from [openrouter.ai/keys](https://openrouter.ai/keys) | Official `GET /api/v1/credits` API. Shows balance and usage in USD. |
| **Hermes Portal** | hermes-agent installed and logged in (`hermes`) — zero extra setup | Reads the Nous OAuth access token from `~/.hermes/auth.json` and calls the portal's billing API (`/api/billing/state` + `/api/billing/subscription`): purchased-credit balance in USD, monthly subscription allowance with tier name and cycle-reset countdown, and monthly-cap usage where configured. The subscription allowance is shown greyed-out and does **not** colour the card or tray while a purchased balance is still funding calls — on the Free tier that allowance is a fraction of a credit and reads 100% used permanently, which is not a quota you're actually hitting. The widget only ever uses the short-lived *access* token — never hermes's refresh token, which the portal rotates and revokes on reuse — so a stale token means "run any `hermes` command" (or keep hermes running; its keepalive refreshes it). **No hermes on this machine?** Set Settings → Hermes → Source to *Remote hermes over SSH* and enter `user@server`: the widget fetches the auth file from a machine that does run hermes (`ssh <host> cat .hermes/auth.json`, BatchMode — needs working key auth; Windows 10/11 include the OpenSSH client). Last resort: paste a portal session cookie. |

Secrets (API key, cookie) are stored in the **Windows Credential Manager**, not
on disk. On Linux dev runs they fall back to a `0600` `secrets.json` in the
config dir. Config lives at `%APPDATA%\quota-widget\config.json`
(`~/.config/quota-widget/` on Linux).

## Building

### CI (recommended)

Push to GitHub — `.github/workflows/windows-build.yml` runs the core test suite
on Linux and produces two artifacts on a Windows runner:

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
binary with `libayatana-appindicator` (dlopened by the tray) and puts
`ssh` on PATH for the Hermes remote source. Bumping npm deps later means
refreshing `npmDeps.hash` in `nix/package.nix`.

### Locally

```sh
npm install
npm run gen-icons          # regenerate src-tauri/icons (already committed)
cargo test -p quota-core   # pure-Rust core: parsers, alert engine, config
npm run tauri dev          # run the app (needs OS webview libs — see below)
npm run tauri build        # produce the exe (run this on Windows)
```

- **On Windows**: install Rust + Node, then the two commands above just work.
- **On Linux** (dev runs): install the Tauri prerequisites first:
  `sudo apt install libwebkit2gtk-4.1-dev build-essential pkg-config libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev`
- **Cross-compiling Windows EXEs from Linux** is possible but officially
  experimental: `cargo install cargo-xwin`, add the `x86_64-pc-windows-msvc`
  target, install `nsis` + `lld`, then
  `npm run tauri build -- --runner cargo-xwin --target x86_64-pc-windows-msvc`.
  Prefer the CI route.

## Deployment

Copy `quota-widget.exe` anywhere and run it. First run: right-click the tray
icon → Settings, enable the providers you use, paste any keys, set thresholds,
and optionally enable **Start with Windows** (registers a `HKCU` run entry via
the autostart plugin — no admin rights needed). Updating = replacing the EXE.

## Architecture

```
crates/quota-core   pure Rust, no UI deps — fully unit-tested
  model.rs          UsageSnapshot / UsageWindow / Credits / FetchError
  config.rs         config persistence + per-provider overrides
  alerts.rs         edge-triggered alert engine
  providers/        one adapter per provider behind a common trait
src-tauri           the Tauri shell
  tray.rs           runtime-generated status icons, menu, popup placement
  poller.rs         poll loop → state → tray/toasts/events
  secrets.rs        Credential Manager (Windows) / 0600 file (elsewhere)
src/                Svelte popup + settings UI
```

## Known limitations

- Claude and Codex usage endpoints are the CLIs' private APIs; a provider-side
  change can break those cards until this widget is updated. The cards degrade
  to a labelled error state rather than crashing.
- The Hermes adapter scans responses leniently for balance-like fields, but the
  portal may change shape; the endpoint URL is user-configurable for that
  reason.
- The Claude "weekly" window's reset cadence is whatever the API reports —
  observed in the wild resetting more often than every 7 days.
