# Quota Widget — releases

Download page for **Quota Widget**, a system-tray widget for Windows 11 and
Linux that watches your AI provider allowances in one place: Claude's rolling
5-hour window and weekly cap, Codex's weekly allowance, Hermes Portal credits,
OpenRouter, ElevenLabs, Firecrawl, DeepSeek and Moonshot balances, and
Fireworks, Anthropic and OpenAI organization spend.

This repository holds **published binaries only** — the source lives elsewhere
and is private. Nothing here is built from code you can read, so install it only
if you trust its author.

## Download

Grab the newest build from [**Releases**](../../releases/latest). Each release
carries:

| Asset | What it is |
|---|---|
| `QuotaWidget_<version>_x64-setup.exe` | NSIS installer — Start Menu entry and an uninstaller. Installs per-user, so no admin prompt. |
| `QuotaWidget_<version>_x64-portable.exe` | The single self-contained EXE. Put it anywhere and run it; nothing is installed. |
| `QuotaWidget_<version>_x64-setup.exe.sig` | Minisign signature over the installer, used to verify updates. |
| `latest.json` | Update manifest the app itself reads. Not something you download by hand. |

Windows 11 ships the WebView2 runtime the UI renders with, so there is no
separate runtime to install.

**Linux** is not distributed here. It is packaged as a Nix flake in the source
repo and updates with `nix profile upgrade`.

## First run

Right-click the tray icon → **Settings**, enable the providers you use, paste
any API keys, and set your thresholds. Optionally turn on **Start on login** — a
`HKCU` run entry on Windows, needing no admin rights.

Secrets (API keys, cookies, OAuth tokens) go into the **Windows Credential
Manager**, never to disk in plaintext. Configuration lives at
`%APPDATA%\quota-widget\config.json`.

## Updates

The app checks this repository's `latest.json` at startup and every six hours,
and shows an unobtrusive "Update available" line in Settings with a **Check
now** button. Uncheck **Check for updates** in Settings to turn the automatic
checks off; **Check now** keeps working either way.

Right now the app *tells* you an update exists and links you here — you download
and run the new installer yourself. In-place installing is coming in a later
version.

Builds made from a development branch carry a branch badge next to the version
and never check for updates at all, since they are usually ahead of the latest
release rather than behind it.

## Verifying a download

Releases are signed with minisign. To check an installer yourself, take the
`signature` value for your platform out of `latest.json` and verify it against
the public key baked into the app:

```
RWQHEg24HhWu6QFITG26y7995k+xW1CG3IHAplDddbIF1LahMc7G7fsz
```

Today the app only reads the manifest to compare version numbers; the signature
is published so that in-place updating can verify it when that ships, and so
you can check a download by hand in the meantime.
