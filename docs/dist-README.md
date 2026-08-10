# Quota Widget — releases

Download page for **Quota Widget**, a system-tray widget for Windows 11 and
Linux that watches your AI provider allowances in one place: Claude's rolling
5-hour window and weekly cap, Codex's weekly allowance, Hermes Portal credits,
OpenRouter, ElevenLabs, Firecrawl, DeepSeek, Moonshot and Venice balances,
and Fireworks, Anthropic and OpenAI organization spend.

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

### Linux

**No Linux download is published here, and the app cannot update itself on
Linux.** The releases above are Windows-only.

This is not an oversight. A Linux build links GTK3 and WebKitGTK from the host
system rather than bundling them the way the Windows EXE bundles nothing at
all, so a single "portable" Linux binary would fail on any distribution whose
library versions differ from the one it was built on — which is most of them.
The supported Linux route is instead a Nix flake, which pins those libraries
exactly.

The flake lives in the source repository, which is private, so it is currently
available only to people who already have access to it. If you are reading this
and want to run it on Linux, open an issue — a public flake or an AppImage is
straightforward to add, it simply has not been needed yet.

The app's own update check is Windows-only for the same reason: on Nix, upgrade
with `nix profile upgrade`.

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

On Windows an **Install update** button appears only for an installed bundle:
the app downloads the new installer, verifies its signature, and runs it. The
app closes and reopens partway through, which is expected. A portable EXE cannot
replace itself, so it instead shows the normal upgrade guidance; update it by
downloading the new one.

Where a release publishes nothing for your platform, the app still tells you a
newer version exists but offers no install button, since it has nothing it could
install. Upgrade however you installed it.

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

The app verifies this signature itself before running a downloaded installer, so
checking by hand is only for the curious or the cautious.
