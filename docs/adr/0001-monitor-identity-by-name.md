# The mini summary's preferred monitor is stored by name, and outlives the monitor

The mini summary can be pinned to a screen, so its anchor has to name one in
`config.json`. Tauri's `Monitor` (2.11.5) offers only three things to key on —
`name()`, the monitor's origin `position()`, and `size()`; there is no EDID or
serial, so no truly stable hardware identity exists. We store the name.

Index into `available_monitors()` was rejected because unplugging one monitor
renumbers the rest, silently retargeting the stored choice at a different
screen. Origin position was rejected because rearranging displays in the OS
settings changes it without any hardware changing. A name is a *port*, so it
survives unplug/replug and rearrangement, and only misidentifies in the narrow
case of moving a screen to a different socket (or swapping two identical
screens between sockets).

## Consequences

The stored name is a **preference, not a fact about the current layout**. When
no connected monitor matches — the common laptop-undock case — the summary
shows on the primary monitor's matching corner and the stored name is left
alone, so reconnecting restores it without the user re-dragging. This means
`config.json` routinely names a monitor that is not present, and the Settings
picker deliberately lists that absent monitor as the current selection rather
than showing the monitor actually in use. Code that reads the anchor must
resolve it against the live monitor list every time; it must never assume the
stored name exists, and must never write the fallback back to config.

`Monitor::name()` is `Option`, so a monitor with no name cannot be pinned at
all; such a drop stores no preference and keeps the previous one.
