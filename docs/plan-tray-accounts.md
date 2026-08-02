# quota-widget: account cards, ordering, mini-summary metrics, and tray tooltip

## Goal

Finish the account-oriented Settings experience and make the compact
tray-click summary useful and reliable:

- every configured account is an unambiguous Settings card;
- saving an addition, removal, rename, reorder, or mini-summary choice is
  immediately reflected when Settings is reopened;
- user order is the display order everywhere;
- every account can choose its compact-summary headline independently;
- Windows and Linux both show a detailed native tooltip on tray hover, while
  left-click opens the interactive mini-summary.

This document describes the state of the repository now. Earlier tray,
multi-account, `ksni`, and pinning work is already present; do not re-plan or
reimplement it as though it were absent.

## Scope and decisions

### Account identity remains immutable

The config map key is the account key, for example `claude#work`. It is used by
the secret store, snapshots, and alert-engine state. It must never change when
the user changes a label or moves an account. The editable account name only
writes `ProviderConfig.label`.

Removing an account continues to clear its account-specific secrets before
deleting the config entry. Moving an account changes only order; it neither
rekeys nor copies secrets.

### Settings order is canonical

`Config.providers` is currently a `BTreeMap`, which sorts account keys and
therefore cannot preserve a user-selected order. Replace it with a
serde-enabled `IndexMap<String, ProviderConfig>`.

JSON object order then becomes the saved account order. Existing config files
deserialize in their existing textual order; configs historically written from
the `BTreeMap` simply retain their old sorted order until the user reorders
them. No migration or secret rewrite is required.

`providers_for()` already drives the poller, `get_snapshots`, and snapshot
events. Once it iterates the `IndexMap`, the full popup, mini-summary,
tooltips, and Settings list can all use the same order.

### Mini-summary statistic is a per-account selection

The chosen headline applies only to the compact mini-summary. It does **not**
change the full provider card, alert thresholds, tray status/gauge, or detailed
hover tooltip.

Add `ProviderConfig.mini_summary_metric: Option<String>`:

- `None` means **Automatic**, preserving current behaviour: choose the worst
  non-informational quota window, otherwise credit balance.
- `credits` selects a credit balance.
- `window:<metric_id>` selects a particular usage window.

Do not persist display text as the choice. Add a backward-compatible
`UsageWindow.metric_id: String` and have adapters supply stable identifiers:

| Provider | Metrics exposed to the mini-summary |
| --- | --- |
| Claude | `five_hour`, `weekly`, and any additional returned limits |
| Codex | `weekly`, plus any other rate-limit window the API currently returns |
| OpenRouter | `credits` |
| Hermes Portal | `credits`, `monthly_cap`, `monthly_allowance` |

Hermes’s monthly cap and monthly allowance are distinct choices. The latter is
currently labelled with the subscription tier, such as `Monthly allowance
(Plus)`, so matching its changing display label would be incorrect. Selecting
an informational Hermes allowance is allowed: the choice changes a headline,
not status or alert semantics.

If a provider temporarily does not return a selected metric, the mini-summary
falls back to Automatic rather than rendering blank. Fetch/auth errors remain
visible as errors.

### Tray interaction

The detailed `poller::tooltip_line()` output is the one source for hover text:
all reported quota windows and credits are included.

- **Windows:** restore the native multiline tray tooltip and remove the custom
  hover-peek window completely, so no two hover surfaces disagree or overlap.
- **Linux/KDE Plasma:** keep the existing `ksni` StatusNotifier tooltip and
  left-click activation unchanged; it already consumes the same detailed text.
- **Both:** left-click toggles the interactive mini-summary. Its existing pin
  behaviour remains session-only and out of scope for this change.

## Implementation work

### 1. Synchronize saved configuration in the frontend

Files: `src/App.svelte`, `src/lib/Settings.svelte`

`set_config` already saves the config, replaces `AppState.config`, emits a
global `config` event, and wakes the poller. `App.svelte` currently does not
consume that event, leaving `appConfig` stale after Settings closes. This is
why newly added accounts and removed OpenRouter accounts appear to come back.

- Listen for `config` alongside the existing snapshot/navigation listeners and
  replace `appConfig` with the event payload; clean the listener up on unmount.
- Keep Settings’ local editable config as `$state.snapshot(initialConfig)`;
  do not clone a Svelte proxy with `structuredClone`.
- Save before closing. A normal test/persist action may emit `config`, but it
  must not discard the active Settings component’s unsaved local edits.

### 2. Build clearly bounded provider account cards

Files: `src/lib/Settings.svelte`, `src/styles.css`

- Keep the outer Providers section and its Add account control.
- Make every `.provider` an individually bordered, padded, rounded card with
  separation from its neighbours.
- Place enabled state, account label, Test, and Up/Down buttons in a card
  header. Do not nest buttons inside a checkbox label.
- Keep credentials, provider-specific configuration, tray inclusion, and the
  new mini-summary selector in the card body.
- Put Remove account in a card footer. It must be inside that card, not after
  the closing element as it is today.
- Add accessible Move up and Move down actions, disabled for the first and
  last account. Rebuild `config.providers` from ordered entries using
  proxy-safe snapshots, `splice`, and `Object.fromEntries`.

### 3. Persist and propagate account order

Files: root `Cargo.toml`, `crates/quota-core/Cargo.toml`, `Cargo.lock`,
`crates/quota-core/src/config.rs`, `crates/quota-core/src/providers/mod.rs`,
`src/lib/Settings.svelte`

- Add `indexmap` with its `serde` feature to the workspace/core dependencies.
- Change the config field and default construction from `BTreeMap` to
  `IndexMap`.
- Keep `providers_for()` in map iteration order and document it as display
  order, not merely stable registry order.
- Verify `poller`’s `join_all` and `get_snapshots` retain this input order; no
  `HashMap` iteration may be used to construct a display list.

### 4. Add stable metric identifiers and configuration

Files: `crates/quota-core/src/model.rs`, `crates/quota-core/src/config.rs`,
`crates/quota-core/src/providers/{claude,codex,hermes,openrouter}.rs`

- Add `metric_id` to `UsageWindow`, with serde/default support so old snapshot
  and test data remain accepted.
- Emit IDs when parsing provider responses. Preserve the existing human-facing
  `label`; do not change user-visible wording just to support selection.
- Add the optional `mini_summary_metric` config field. Missing values keep
  Automatic behaviour, so existing config files remain compatible.
- Claude should identify its five-hour and weekly windows independently,
  including any distinct per-model weekly limits. Codex should identify windows
  from their reported duration rather than assume its API will always expose
  only weekly. Hermes must explicitly emit `monthly_cap` and
  `monthly_allowance`.

### 5. Expose selected metrics in Settings and the mini-summary

Files: `src/App.svelte`, `src/lib/Settings.svelte`,
`src/lib/MiniSummary.svelte`, `scripts/smoke-mount.mjs`

- Pass current snapshots from App into Settings so each account can show
  choices actually returned by the provider, including future API windows.
- Offer the known choices even before a first successful poll:
  - Claude: Automatic, 5-hour, Weekly.
  - Codex: Automatic, Weekly; add currently returned alternatives when present.
  - OpenRouter: Automatic, credit balance.
  - Hermes: Automatic, purchased credit balance, Monthly cap, Monthly
    allowance.
- Deduplicate static and live choices by metric ID. Show the live window label
  for any newly reported provider metric.
- Refactor MiniSummary’s selection helper to resolve the account setting,
  selected credits/window, and Automatic fallback. It should deliberately
  allow a selected informational window.
- Preserve `mini_summary_bars` as the global switch controlling whether the
  selected percentage metric has a bar. It is independent from selecting the
  metric itself.

### 6. Fix the mini window’s capability and error state

Files: `src-tauri/capabilities/mini.json` (new),
`src/lib/MiniSummary.svelte`, `scripts/smoke-mount.mjs`

The `mini` window is not assigned to a capability: `default.json` applies only
to `main`, and `hover.json` only to `hover`. Its invokes/events can therefore
be denied, which explains the blank summary.

- Add a narrow `mini` capability with `windows: ["mini"]`, `core:default`, and
  `core:event:default`. Rust performs all positioning/topmost work, so no
  additional frontend window-management permission is needed.
- Catch a failed initial `get_snapshots` call and display an explicit loading
  or error state, never an empty-looking summary.
- Update smoke fixtures with `metric_id` fields and assert that MiniSummary
  mounts with snapshot data and can render its error state.

### 7. Replace Windows custom hover with the native tooltip

Files: `src-tauri/src/tray.rs`, `src-tauri/tauri.conf.json`,
`src-tauri/capabilities/hover.json` (delete), `src/main.js`,
`src/lib/HoverSummary.svelte` (delete), `src/styles.css`,
`scripts/smoke-mount.mjs`, `README.md`

- In non-Linux `tray::set_status`, set both the status icon and native tooltip
  from the poller-provided multiline string. The current parameter is ignored.
- Remove Windows `TrayIconEvent::Enter`/`Leave`, `show_hover`, `hide_hover`,
  and calls that only exist to hide that window.
- Remove the `hover` Tauri window, its capability, frontend route/import/body
  class, hover component, hover-only CSS, and smoke-mount case.
- Do not change `tray_linux.rs` or the Linux `poller.rs` dispatch: `ksni`
  already publishes the same string as the native SNI tooltip and maps left
  activation to `toggle_mini`.
- Update README’s platform table, interaction description, architecture list,
  and caveats to describe native detailed tooltips on both platforms.

### 8. Version and documentation

Files: root `Cargo.toml`, `README.md`, this document

- Bump the application version in the workspace `Cargo.toml` only, following
  the repository’s single-source version rule.
- Do not change `package.json` dependencies; no `npmDeps.hash` regeneration is
  expected.
- Keep README accurate about mini-summary selection, account ordering, and the
  Windows/Linux hover implementation.

## Parallel implementation protocol

The workspace is shared. Parallel agents must avoid overlapping edits and must
not use destructive Git commands, change remotes/credentials, push, or bump
the version independently. The coordinating agent performs final integration,
the version bump, documentation reconciliation, and the complete test pass.

### Codex workstream — config and Settings

Owns:

- `Cargo.toml`, `crates/quota-core/Cargo.toml`, `Cargo.lock`
- `crates/quota-core/src/config.rs`, `model.rs`, provider parsers, and core
  provider tests
- `src/App.svelte` and `src/lib/Settings.svelte`

Tasks:

1. Introduce `IndexMap`, persistent order, `metric_id`, and
   `mini_summary_metric` with backward-compatible defaults.
2. Implement metric IDs in all adapters, especially Hermes cap versus
   allowance.
3. Fix App’s `config` event synchronization, account cards, add/remove
   persistence, selector UI, and Up/Down ordering.
4. Add core and frontend smoke fixtures/tests for order and metric choices.

Constraints:

- Never rekey accounts while renaming or ordering them.
- Use `$state.snapshot` at Svelte proxy boundaries.
- Do not edit tray/capability files owned by the Claude workstream.

### Claude workstream — tray and mini runtime

Owns:

- `src-tauri/src/tray.rs`
- `src-tauri/capabilities/mini.json`
- `src-tauri/tauri.conf.json` and deletion of `hover.json`
- `src/lib/MiniSummary.svelte`, `src/main.js`, and deletion of
  `src/lib/HoverSummary.svelte`

Tasks:

1. Grant the mini window its minimal invoke/event capability and give it a
   visible initial-load failure state.
2. Restore the detailed native Windows tooltip and remove the custom hover
   window path completely.
3. Preserve existing Linux `ksni` behaviour and left-click mini-summary
   activation.

Constraints:

- Do not modify config/model/parser logic or Settings account ordering.
- Do not weaken capability scope beyond what the mini window actually needs.
- Coordinate stylesheet and smoke-script cleanup with the integrator to avoid
  concurrent edits to shared frontend files.

### Integration order

1. Codex lands the config/model/UI changes and reports the exact new metric
   payload shape.
2. Claude lands the capability/tray changes against that shape.
3. The coordinating agent resolves any overlap in `styles.css`,
   `scripts/smoke-mount.mjs`, README, and this plan; bumps the version; then
   runs the full verification suite.

## Verification

Automated checks:

- `cargo test -p quota-core`
  - old config with no order/metric fields still loads;
  - save/load preserves an explicit account order;
  - `providers_for()` preserves that order;
  - labels do not affect keys;
  - Claude/Codex/Hermes parser metric IDs are stable;
  - Hermes exposes distinct monthly-cap and monthly-allowance IDs;
  - selected metric, informational allowance, unavailable-metric fallback, and
    Automatic selection behave correctly.
- `cargo check -p quota-core`.
- `npm run build`.
- `npm run check-versions`.
- `npm i -D jsdom --no-save && npm run smoke-mount`.
- Build/check the Tauri crate when local `clang`/`lld` are available; otherwise
  report that limitation plainly. Capability JSON must be schema-validated by
  the Tauri build/check path.

Manual Windows checks:

1. Hover shows one native multiline tooltip containing all provider windows
   and balances; no custom hover window appears.
2. Left-click toggles the mini-summary; right-click keeps the normal menu.
3. Toggle bars, then select Claude 5-hour versus Weekly and confirm only the
   mini-summary headline changes.
4. Confirm Hermes can select credit balance, Monthly cap, and Monthly
   allowance independently.
5. Add, remove, rename, reorder, Save, reopen Settings, and restart the app;
   all changes persist and popup/mini order matches Settings.

Manual KDE Plasma checks:

1. Hover shows the detailed Plasma SNI tooltip.
2. Left-click toggles the mini-summary and right-click opens the menu.
3. The mini-summary order and selected headlines match Settings.
4. Existing pin/unpin, focus-loss, and XWayland placement behaviour remains
   unchanged.

Do not push. Commit locally only when the implementation is complete and let
Ian explicitly choose whether to run the Windows CI build.
