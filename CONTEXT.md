# Quota Widget

A tray widget that reports how much of each AI provider's allowance you have
left. This glossary fixes the vocabulary for the quantities it displays, which
look interchangeable on screen and are not.

## Language

**Usage window**:
A share of an allowance that refills on a period — Claude's rolling 5 hours,
a weekly cap, a calendar month against a budget. It is a *percentage*, never
an amount: the underlying used and granted figures are not retained.
_Avoid_: Quota, limit, allowance window

**Credits**:
An *amount* of money or credit attached to an account, in its own unit. Has no
period and no percentage, so it is reported as a figure rather than a bar.
_Avoid_: Balance (that is one kind of credits — see below), funds

**Balance**:
Credits that remain and can still be spent. Reads as a bare figure, because
that is what money left looks like.
_Avoid_: Remaining credits, available funds

**Spend**:
Credits already consumed in the current calendar month. Distinguished from a
balance by carrying a name for what the figure is, so money spent is never
presented as money remaining.
_Avoid_: Cost, usage, burn

**Period marker**:
How far through a usage window's period the present moment is, drawn on that
window's bar. It shares the bar's x-axis with the usage fill but measures a
different quantity — time elapsed, not allowance consumed — which is why a
half-full bar past the marker means the allowance is burning fast. For a window
that resets weekly it measures *scheduled* time elapsed under the account's
[[usage schedule]] rather than raw calendar time, freezing on off-days; every
other window measures raw calendar time.
_Avoid_: Progress marker, time marker, tick

**Usage schedule**:
The set of weekdays an account is expected to be used on. It redistributes a
weekly [[period marker]]'s expected pace onto those days alone — five active
days make each worth a fifth of the week — so the marker holds flat on off-days
instead of creeping. It shapes only the expectation, never the usage figure a
provider reports.
_Avoid_: Active days, working days, roster, duty cycle

**Calendar marker**:
The raw, unscheduled position of the present moment in a period — where the
[[period marker]] would sit with no [[usage schedule]] applied. Revealed while a
bar is pressed and held, so a glance answers "where would I be on the plain
calendar?" without leaving the scheduled view.
_Avoid_: True marker, real marker, raw marker

**Informational window**:
A usage window that is shown but never colours a card or the tray, because
exhausting it does not actually block calls.
_Avoid_: Muted window, non-gating window

**Anchor**:
The corner of a monitor's work area that a window is pinned to, and the
monitor it is pinned on. It is a corner rather than a position because the
summary resizes to its content: an edge to grow from is what stops the window
jumping when an account appears. Work area rather than monitor bounds, because
the latter puts the window underneath a panel. Pinning an unanchored summary
selects its nearest anchor for later resizing without first moving the summary.
_Avoid_: Position, placement, location

**Pinning**:
The transition that makes the mini summary follow an anchor. Every transition
from unpinned to pinned adopts the mini summary's current visible placement, so
pinning is not itself a visible move.
_Avoid_: Reposition, reset placement

**Preferred monitor**:
The screen the user chose for a window, stored by monitor name. It is a
*preference*, not a fact about the current display layout: when that monitor is
not connected the window is shown elsewhere while the preference stays put, so
reconnecting the monitor restores the window without the user asking again.
_Avoid_: Current monitor (that is what the window is on now), display, screen

**Snap**:
Moving a window to its [[anchor]] after the user drops it somewhere that is not
a corner. A drop states an intent — *this screen, roughly here* — and the snap
resolves it to the nearest corner, so what is on screen always matches what is
stored.
_Avoid_: Align, dock, reposition

**Distribution artifact**:
A binary published for download — the AppImage, the installer, the portable
EXE. It is built and signed in CI and published to the main repository (the
primary channel) and mirrored to the dist repository during the transition
(ADR-0005), and is a different thing from the source: the source being readable
is not a distribution channel, and a source build is never one of these.
_Avoid_: Build, release, asset, package

**Installable artifact**:
A [[distribution artifact]] the running build can replace *itself* with. It is
decided by the bundle format the process is running as, never by what a release
published: a portable EXE finds the Windows installer in the manifest and still
cannot install it. Anything else gets upgrade guidance instead of a button.
_Avoid_: Updatable build, self-updating build, supported download

**Release signature**:
The minisign signature over a [[distribution artifact]], carried both beside it
as a `.sig` file and inline in the manifest. One signature serves two readers —
a person verifying a download by hand, and the updater verifying before it
writes — so there is no separate "update signature" to keep in step.
_Avoid_: Checksum, hash, update signature

**Tested platforms**:
The platforms a release is actually validated on, replacing the former
"compatibility floor" (dropped in ADR-0004): NixOS + KDE Plasma (source build —
tray and placement), Windows (release build), and Debian 13 XFCE (that the
shipped AppImage binary starts on a mainstream non-Nix distro). The AppImage
carries **no promised minimum distribution**; it is best-effort on glibc-based
`x86_64` Linux, built on a pinned Ubuntu 24.04. A virgl/virtualized-GPU VM is
not a valid environment for testing it.
_Avoid_: Compatibility floor, minimum requirements, baseline, supported distro

**Launcher**:
The single `.desktop` file that puts the app in the applications menu. For the
AppImage it is app-managed and opt-in; for the Nix package it belongs to the
package. It names the AppImage *where the user keeps it*, so an in-place update
leaves it valid and a move breaks it.
_Avoid_: Shortcut, desktop entry (that is the file format), menu item

**Ownership marker**:
What licenses the app to delete a launcher or icon: a marker key in the file
*and* a byte-for-byte match against what would be written for the path the file
itself records. Both together, because either alone would let an edited file be
destroyed.
_Avoid_: Signature, checksum, tag

**Repair**:
Retargeting an app-owned launcher whose AppImage has moved. Always offered,
never silent — the file is the user's and may have been pointed somewhere
deliberately.
_Avoid_: Fix, update, refresh

**Deferral**:
The record that the first-run integration question has been *asked*. It stores
that the user was asked, not what they answered, so declining sticks as firmly
as accepting and no launch nags.
_Avoid_: Dismissal, opt-out, snooze

**Account**:
One configured sign-in to a provider, with its own name, secrets and settings.
A provider may have several, and each is ordered and displayed independently.
_Avoid_: Provider (the vendor), profile, connection

**Android app**:
The full-screen Android form of Quota Widget, where a user views detailed
usage, manages accounts and changes settings.
_Avoid_: Android widget, mobile widget

**Home-screen widget**:
The compact Quota Widget surface placed in an Android launcher's home screen.
It is a companion surface of the standalone [[Android app]], not the app itself.
_Avoid_: Android app, mobile app, desktop widget

**Outcome parity**:
The cross-platform promise that accounts, quota meanings, ordering, thresholds,
status and visual identity agree while each platform uses its own native
surfaces and interactions.
_Avoid_: Pixel parity, interaction parity, identical experience

**Background refresh target**:
The interval Android asks for between quota refresh opportunities while the app
is not in use. It is a best-effort request to the operating system, never a
promise that a refresh will run at that time.
_Avoid_: Poll interval, refresh guarantee, refresh schedule

**Widget instance**:
One placement of the [[home-screen widget]], with its own selected accounts and
headlines. A user may place several independently configured instances.
_Avoid_: Android widget, widget type, copy

**Credential source**:
The way an account supplies authentication, such as built-in sign-in, a pasted
secret or a desktop CLI login. A provider may exist on every platform without
every credential source existing there.
_Avoid_: Provider, account, authentication method

**Personal Android build**:
A consistently signed APK produced on demand for the owner's direct install.
It is not a public release or a [[distribution artifact]].
_Avoid_: Android release, debug build, Play Store build

**Android application identity**:
The permanent package identity `tech.allaway.quotawidget`, shared by every
personal Android build so later APKs update the same installed application.
_Avoid_: Desktop identifier, app name, signing identity

**Android validation target**:
The real device and launcher on which the personal Android build must be
manually proven: a Google Pixel 7 running Android 17 with Pixel Launcher.
_Avoid_: Android support matrix, emulator, minimum API

**Pending sign-in**:
The short-lived state of a built-in browser sign-in that has started but not
finished. It survives the Android app leaving memory, but only until the
provider's flow expires.
_Avoid_: Login session, account, saved sign-in

**Shared configuration**:
The accounts, provider settings, thresholds, alerts, ordering and headline
choices whose meaning is common to every Quota Widget platform.
_Avoid_: Config file, platform settings, synced configuration

**Platform preferences**:
Choices about behaviour that exists on only one host, such as desktop window
placement or Android home-screen presentation.
_Avoid_: Shared configuration, provider settings

**Widget privacy mode**:
A per-[[widget instance]] choice that hides quota figures and balances while
retaining account names and status colours.
_Avoid_: Private account, secret widget, lock-screen mode

**Trusted endpoint**:
A provider endpoint reached over HTTPS whose certificate is accepted by the
platform's system trust store.
_Avoid_: Custom endpoint, secure endpoint, certificate override

**Foreground refresh**:
Quota refreshes performed while the Android app is visible: once immediately
on entry, then at the configured interval until the app leaves the foreground.
_Avoid_: Background refresh, continuous polling, widget refresh

**Stale reading**:
The last successful quota figure retained after a newer refresh fails. It is
visibly aged and unavailable rather than replaced by a blank or current-looking
value.
_Avoid_: Cached value, old data, failed reading

**Configuration draft**:
The editable copy of Settings that remains separate from saved configuration
until the user explicitly saves it. Leaving with changes requires a discard
decision.
_Avoid_: Live settings, autosaved settings, form state

**Provider onboarding**:
The Android first-run path from an empty account list through choosing a
provider and supplying one of its supported [[credential source]]s.
_Avoid_: Default accounts, setup wizard, sign-in

**Alert memory**:
The last alert level successfully evaluated for an account on this Android
installation. It survives process death, reboot and upgrade so those lifecycle
events cannot turn an unchanged critical state into repeated crossings.
_Avoid_: Alert baseline, notification history, cached alert

**Unconfigured widget instance**:
A [[widget instance]] whose saved account selection is missing or unreadable.
It asks to be configured and never silently substitutes another account.
_Avoid_: Empty widget, broken widget, default widget

**Desktop integration**:
A per-user launcher and icon registration that makes a standalone AppImage
discoverable from the desktop environment's application menu.
_Avoid_: System installation, AppImage daemon

**Mini-summary fade level**:
The mini summary's current visual opacity for this running app process. It
survives hiding and reopening the summary, but begins fully opaque on a fresh start.
_Avoid_: Saved opacity, fade preference

**Tray-first launch**:
Starting the application without presenting its main window; the tray icon is
the initial point of access. A transient taskbar icon while the process starts
is not presentation of the main window.
_Avoid_: Background launch, hidden launch

**Explicit activation**:
A deliberate request to interact with an already-running application, such as
launching it a second time. It may present the main window despite a tray-first
launch policy.
_Avoid_: Startup, automatic activation

**Alert baseline**:
The first quota state observed in one running application process. It records
current alert levels without treating them as new threshold crossings.
_Avoid_: Startup alert, initial transition

**Startup alert policy**:
The presentation allowed for alert levels already present at the alert
baseline: warnings remain tray-only, while critical states may notify but do
not present the main window.
_Avoid_: Initial notification setting, launch warning

**Settings return state**:
Where a Settings visit goes when it exits — the usage popup, the mini summary,
or no window at all. It is *transient*: captured when Settings opens and gone
when that visit ends, never written to the config file, because it describes
what was on screen a moment ago rather than anything the user chose. Only the
two exits honour it: Save & close and Esc. ✕ is an explicit hide and ignores
it, and ← Back is navigation within the window rather than an exit at all — it
shows the usage list, ends the visit, and leaves the window on screen.
_Avoid_: Previous view, back target, exit destination, saved view
