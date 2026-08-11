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
half-full bar past the marker means the allowance is burning fast.
_Avoid_: Progress marker, time marker, tick

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

**Distribution artifact**:
A release file published for people to install on one platform, independent of
whether the source repository is public.
_Avoid_: Build, package (unless a platform-specific format is meant), source release

**Installable artifact**:
A distribution artifact the running build can replace through the in-app updater.
An AppImage is installable in place; a Nix build is not.
_Avoid_: Available download, update (which may only be detectable)

**Compatibility floor**:
The oldest operating-system environment a distribution artifact is promised to
run on; newer environments are compatible by implication.
_Avoid_: Build runner, latest Linux

**Release signature**:
The minisign signature over a distribution artifact, used by both the updater
and a person verifying a download.
_Avoid_: Checksum, release key

**Desktop integration**:
A per-user launcher and icon registration that makes a standalone AppImage
discoverable from the desktop environment's application menu.
_Avoid_: System installation, AppImage daemon

**Mini-summary fade level**:
The mini summary's current visual opacity for this running app process. It
survives hiding and reopening the summary, but begins fully opaque on a fresh start.
_Avoid_: Saved opacity, fade preference

**Settings return state**:
The visible UI state to restore when Settings exits: the usage popup, the mini
summary, or neither window. It is captured when Settings opens and lasts only
for that visit; it is not a user preference or saved configuration.
_Avoid_: Previous page, navigation history, saved window state

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
