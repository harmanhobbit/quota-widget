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
the latter puts the window underneath a panel.
_Avoid_: Position, placement, location

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

**Account**:
One configured sign-in to a provider, with its own name, secrets and settings.
A provider may have several, and each is ordered and displayed independently.
_Avoid_: Provider (the vendor), profile, connection

**Settings return state**:
Where a Settings visit goes when it exits — the usage popup, the mini summary,
or no window at all. It is *transient*: captured when Settings opens and gone
when that visit ends, never written to the config file, because it describes
what was on screen a moment ago rather than anything the user chose. Only the
two exits honour it: Save & close and Esc. ✕ is an explicit hide and ignores
it, and ← Back is navigation within the window rather than an exit at all — it
shows the usage list, ends the visit, and leaves the window on screen.
_Avoid_: Previous view, back target, exit destination, saved view
