# Quota Widget — releases

Download page for **Quota Widget**, a system-tray widget for Windows 11 and
Linux that watches your AI provider allowances in one place: Claude's rolling
5-hour window and weekly cap, Codex's weekly allowance, Hermes Portal credits,
OpenRouter, ElevenLabs, Firecrawl, DeepSeek, Moonshot and Venice balances,
and Fireworks, Anthropic and OpenAI organization spend.

This repository holds **published binaries only**. The source lives in
[`harmanhobbit/quota-widget`](https://github.com/harmanhobbit/quota-widget), is
public, and is licensed Apache-2.0; every asset here is built and signed by that
repository's release workflow.

## Download

Grab the newest build from [**Releases**](../../releases/latest). Each release
carries:

| Asset | What it is |
|---|---|
| `QuotaWidget_<version>_x64-setup.exe` | NSIS installer — Start Menu entry and an uninstaller. Installs per-user, so no admin prompt. |
| `QuotaWidget_<version>_x64-portable.exe` | The single self-contained EXE. Put it anywhere and run it; nothing is installed. |
| `QuotaWidget_<version>_x64-setup.exe.sig` | Minisign signature over the installer, used to verify updates. |
| `QuotaWidget_<version>_amd64.AppImage` | Standalone Linux x86_64 application image. |
| `QuotaWidget_<version>_amd64.AppImage.sig` | Minisign signature over the AppImage. |
| `latest.json` | Update manifest the app itself reads. Not something you download by hand. |

Windows 11 ships the WebView2 runtime the UI renders with, so there is no
separate runtime to install.

### Linux

Download the x86_64 `.AppImage` and its adjacent `.sig` file from the release
you want. It is built on a pinned **Ubuntu 22.04** runner, which is the
compatibility floor: use Ubuntu 22.04 or a newer compatible glibc-based Linux
distribution. Make it executable and launch it directly:

```sh
chmod +x QuotaWidget_<version>_amd64.AppImage
./QuotaWidget_<version>_amd64.AppImage
```

The AppImage is the public direct-download route. The Nix flake in the source
repository is a separate, reproducible packaging route: it pins the GTK/WebKit
runtime and builds from source rather than downloading this release asset, so it
takes no part in in-app updates. On Nix, keep upgrading with
`nix profile upgrade`.

## How far each provider has been tested

Every adapter is written against the vendor's documented response schema and
covered by unit tests over that schema. What follows is what has additionally
been seen from a **live account** — worth knowing before you trust a number on
screen.

- **Verified with real usage data:** Firecrawl. Its parse path has run on a
  meaningful non-zero reading, so the arithmetic and formatting are exercised,
  not just the plumbing.
- **Reached successfully, but only on an account reporting $0.00:** DeepSeek,
  Moonshot, OneHop, Fireworks, OpenAI Admin. The key is accepted, the endpoint
  is right and the response parses — but a zero says nothing about whether a
  non-zero amount is scaled correctly.
- **Not yet run against a live account:** Anthropic Admin, Venice.

That middle distinction is not pedantry. Anthropic's cost report returns its
amount in **cents** while OpenAI's returns **dollars**, so a units mistake is a
100x error that a $0.00 reading cannot possibly reveal. Treat the first non-zero
figure from any provider in the lower two groups as worth checking against that
vendor's own dashboard.

Separately, a few endpoints are **unofficial** — Claude and Codex use the same
private APIs their CLIs call, and OneHop's balance endpoint is undocumented.
These work today and may stop working without notice.

## First run

Right-click the tray icon → **Settings**, enable the providers you use, paste
any API keys, and set your thresholds. Optionally turn on **Start on login** — a
`HKCU` run entry on Windows, needing no admin rights.

### Where your API keys go

Secrets (API keys, cookies, OAuth tokens) go into the **Windows Credential
Manager**, never to disk in plaintext. Configuration lives at
`%APPDATA%\quota-widget\config.json`, which holds no secrets.

Credential Manager encrypts entries with DPAPI, tied to your Windows account.
Another user on the same machine cannot read them, and they are not recoverable
from a stolen disk or a file-level backup. They *are* readable by anything
running as you, since DPAPI unwraps automatically for your own session — the
same model the GitHub CLI and the Claude and Codex CLIs use. So this protects
against other accounts and offline access, not against malware you are running.

The keys never leave the app except to the provider they belong to. They are
not sent anywhere else, and the app's own update check talks only to this
repository's release manifest and carries no credentials.

Worth judging what you paste in accordingly: a read-only usage key is a very
different prospect from an organization admin key, and the Anthropic Admin and
OpenAI Admin providers deliberately require the latter.

## Updates

The app checks this repository's `latest.json` at startup and every six hours,
and shows an unobtrusive "Update available" line in Settings with a **Check
now** button. Uncheck **Check for updates** in Settings to turn the automatic
checks off; **Check now** keeps working either way.

An **Install update** button appears when the app is running as something it
can replace. The download's signature is always verified before anything is
run or written.

- **Windows, installed via the installer**: the app downloads the new
  installer and runs it. The app closes and reopens partway through, which is
  expected.
- **Linux, running the AppImage**: the app downloads the new AppImage and
  replaces the one you launched. Nothing restarts on its own, so it then offers
  **Restart now** and **Later**. Choosing Later does not undo anything — the
  new version is already in place and starts the next time you open the app.

A portable EXE cannot replace itself while running, so it shows the normal
upgrade guidance instead; update it by downloading the new one. The same is
true of a package-managed install such as Nix — upgrade it with your package
manager (`nix profile upgrade quota-widget`).

Where a release publishes nothing for your platform, the app still tells you a
newer version exists but offers no install button, since it has nothing it could
install. Upgrade however you installed it.

Builds made from a development branch carry a branch badge next to the version
and never check for updates at all, since they are usually ahead of the latest
release rather than behind it.

## Verifying a download

Releases are signed with minisign. To manually verify an AppImage, download its
matching `.sig` file as well, install `minisign` (for example `sudo apt install
minisign` on Ubuntu), and verify with the public key baked into the app:

```
RWQHEg24HhWu6QFITG26y7995k+xW1CG3IHAplDddbIF1LahMc7G7fsz
```

```sh
minisign -Vm QuotaWidget_<version>_amd64.AppImage \
  -x QuotaWidget_<version>_amd64.AppImage.sig \
  -P RWQHEg24HhWu6QFITG26y7995k+xW1CG3IHAplDddbIF1LahMc7G7fsz
```

The app verifies this signature itself before installing an in-app update, so
checking a direct download by hand is for the curious or the cautious.
