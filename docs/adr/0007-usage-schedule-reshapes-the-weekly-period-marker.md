# Usage schedule reshapes the weekly period marker, and only the marker

An account may carry a usage schedule — the weekdays it is expected to be used
on — and for a window that resets weekly the period marker then measures
scheduled time elapsed instead of raw calendar time, freezing on off-days so a
Monday–Friday account paces at a fifth of the allowance per working day. The
schedule is purely presentational: it never alters the used percentage a
provider reports, and status and alerts continue to key off that raw figure.

## Considered options

A second, separate "your-days" marker drawn alongside the provider's true one
was rejected: two lines on a thin bar are hard to read, and the calendar
position is rarely what the user is asking about. Making the schedule discount
real usage — so weekend spend "wouldn't count" — was rejected because it cannot:
the provider debits the weekly cap the moment tokens are spent and the widget
only reads the figure back, so discounting it would display a false allowance.
Feeding the schedule into alerts was deferred rather than rejected; once the
marker exists, an "ahead of your working-days pace" alert is a clean follow-up.

## Consequences

Only weekly-resetting windows are affected, identified by the stable
`window:weekly` metric identity rather than by inferring cadence from the
period's span or matching a display label; a window whose identity is absent or
unrecognised keeps the raw calendar marker. Because the raw position is still
worth seeing, pressing and holding a bar reveals the calendar marker and
releasing reverts — a momentary peek, not a stored mode. The default schedule is
all seven days, which is identical to the previous behaviour, so existing
accounts are unchanged until they opt in. Day boundaries follow the device's
local time and partial boundary days count fractionally. The schedule is part of
the shared-configuration schema and is therefore stored independently by each
platform, with no cross-device syncing.
