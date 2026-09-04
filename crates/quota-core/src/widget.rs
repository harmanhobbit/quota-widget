//! The home-screen widget read model.
//!
//! An Android launcher hosts one or more [`Widget instance`s](../../CONTEXT.md),
//! each an independent placement with its own selected accounts, headline
//! choices and [`Widget privacy mode`](../../CONTEXT.md). A widget never
//! fetches and never holds credentials (`docs/adr/0006-…`): it renders purely
//! from the persisted [`crate::snapshots::SnapshotStore`] a refresh left behind,
//! projected through the instance's saved selection into exactly what the
//! launcher should draw at its current size.
//!
//! This module is that projection — the part issue #113's acceptance criteria
//! call out as unit-tested behaviour (breakpoint selection, per-instance
//! config, privacy redaction, removed-account state). The Glance/RemoteViews
//! rendering, the WorkManager one-time refresh, and the deep-link `Intent` are
//! thin host wiring on top of what [`project`] returns, and live in the native
//! Android host the same way the desktop tray's drawing lives outside
//! quota-core. Keeping the decision here is what stops the widget from drifting
//! into "a new interpretation" of a quota the shared surfaces already settled.
//!
//! ## What the widget must never do
//!
//! - **Never substitute an account.** A selected account that has disappeared
//!   from the read model becomes an explicit [`RowState::Removed`], never
//!   another account silently taking its place.
//! - **Never reinterpret the status colour.** The widget colours itself from
//!   the shared [`AggregateStatus`] the refresh already folded and persisted
//!   ([`WidgetContent::aggregate`]), not from a fold recomputed over the
//!   instance's subset of accounts.
//! - **Never invent data.** An unconfigured instance, a corrupt preference
//!   file, and an empty read model each resolve to their own honest placeholder
//!   ([`WidgetState`]), not a fabricated reading.

use crate::config::{Config, UsageSchedule};
use crate::model::{Allowance, Credits, Status, UsageSnapshot, UsageWindow};
use crate::refresh::AggregateStatus;
use crate::snapshots::SnapshotStore;
use chrono::{DateTime, Datelike, Duration, Local, LocalResult, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// The responsive size class a launcher's provided dimensions fall into. The
/// three tiers deliberately mirror the acceptance criteria: [`Small`] is an
/// aggregate glance, [`Medium`] adds per-account rows, [`Large`] adds usage
/// bars and reset information.
///
/// [`Small`]: WidgetSize::Small
/// [`Medium`]: WidgetSize::Medium
/// [`Large`]: WidgetSize::Large
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WidgetSize {
    /// Aggregate status + the single worst selected headline + data age. A
    /// 1x1-ish cell where per-account rows would not fit without cropping.
    Small,
    /// The selected account rows, each with its value and status.
    Medium,
    /// The medium rows plus a usage bar and reset information per headline.
    Large,
}

impl WidgetSize {
    // Breakpoints in density-independent pixels. Both dimensions must clear a
    // tier's floor, so a tall-but-narrow or wide-but-short cell steps *down*
    // rather than cropping content it has no room for — "resizing does not
    // crop/stretch" is served by choosing a tier that fits, not by squeezing a
    // richer tier into a smaller box. The numbers bracket the launcher grid a
    // phone actually produces: a medium tier needs room to stack a couple of
    // rows; a large tier needs the extra height a bar plus a reset line adds.
    const MEDIUM_MIN_WIDTH_DP: f64 = 180.0;
    const MEDIUM_MIN_HEIGHT_DP: f64 = 110.0;
    const LARGE_MIN_WIDTH_DP: f64 = 250.0;
    const LARGE_MIN_HEIGHT_DP: f64 = 200.0;

    /// Choose the richest tier that fits within the launcher-provided box. A
    /// non-finite or non-positive dimension (a launcher can report either
    /// mid-resize) is treated as too small for that axis, so the widget degrades
    /// to [`Small`](WidgetSize::Small) rather than picking a tier off garbage.
    pub fn from_dimensions(width_dp: f64, height_dp: f64) -> Self {
        let fits = |w: f64, h: f64| {
            width_dp.is_finite() && height_dp.is_finite() && width_dp >= w && height_dp >= h
        };
        if fits(Self::LARGE_MIN_WIDTH_DP, Self::LARGE_MIN_HEIGHT_DP) {
            Self::Large
        } else if fits(Self::MEDIUM_MIN_WIDTH_DP, Self::MEDIUM_MIN_HEIGHT_DP) {
            Self::Medium
        } else {
            Self::Small
        }
    }

    /// Whether this tier lays out per-account rows at all. The small tier
    /// collapses to [`WidgetContent::worst`] instead.
    fn shows_rows(self) -> bool {
        !matches!(self, Self::Small)
    }

    /// Whether this tier carries a usage bar and reset time on each headline.
    fn shows_bars_and_reset(self) -> bool {
        matches!(self, Self::Large)
    }
}

/// One account's place in a [`Widget instance`](../../CONTEXT.md)'s selection:
/// which account, and which headlines to show for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetAccountSelection {
    /// The account (its `Config::providers` key) this selection names.
    pub provider_id: String,
    /// The headlines to show for this account, or `None` to inherit the shared
    /// compact-summary selection ([`Config::resolved_mini_metrics`]). Held
    /// per-instance so one widget can pin a different headline than another
    /// showing the same account — a `Some` here overrides the shared choice for
    /// this instance only, never mutating it.
    #[serde(default)]
    pub headlines: Option<Vec<String>>,
}

/// A single [`Widget instance`](../../CONTEXT.md)'s saved configuration: its own
/// account/headline selection and its own [`Widget privacy mode`]. Two
/// instances are independent owned values, so configuring one never touches
/// another that happens to show the same accounts.
///
/// [`Widget privacy mode`]: ../../CONTEXT.md
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WidgetInstanceConfig {
    /// The accounts this instance shows, in the order they were selected.
    pub accounts: Vec<WidgetAccountSelection>,
    /// Hide quota figures and balances while keeping account names and status
    /// colours ([`Widget privacy mode`](../../CONTEXT.md)).
    pub privacy: bool,
}

impl WidgetInstanceConfig {
    /// Seed a brand-new instance from the shared compact-summary selection, the
    /// initial choice a placement inherits (issue #113: "may inherit the shared
    /// compact-summary selection as its initial choice"). Every enabled account
    /// that has not opted out of the compact summary (an empty
    /// [`Config::resolved_mini_metrics`]) is included, in configured order, each
    /// inheriting its shared headlines (`headlines: None`). The result is a
    /// plain owned value the caller is free to diverge afterwards — inheriting
    /// copies the choice, it does not bind the instance to the shared one.
    pub fn inherit_shared(cfg: &Config) -> Self {
        let accounts = cfg
            .providers
            .iter()
            .filter(|(_, p)| p.enabled)
            .filter(|(id, _)| {
                // An explicit empty selection is an account opting out of the
                // compact summary; it should not seed a new widget either.
                cfg.resolved_mini_metrics(id)
                    .map(|m| !m.is_empty())
                    .unwrap_or(true)
            })
            .map(|(id, _)| WidgetAccountSelection {
                provider_id: id.clone(),
                headlines: None,
            })
            .collect();
        Self {
            accounts,
            privacy: false,
        }
    }
}

/// The persisted per-installation widget preferences: every placed instance's
/// [`WidgetInstanceConfig`], keyed by the launcher's stable instance id.
///
/// ## Corruption is "needs configuration", by design
///
/// These are platform preferences that belong to one installation
/// (`docs/adr/0006-…`), but they are not user-authored secrets: a missing or
/// unparseable file loads as the empty store, so a placed instance whose entry
/// cannot be found projects [`WidgetState::NeedsConfiguration`] — exactly the
/// acceptance criterion "corrupt/missing prefs → Widget needs configuration".
/// The instance then re-saves itself the next time the user configures it. This
/// is the opposite of `shared-config.json`, whose unparseable form is *kept* and
/// blocks replacement because its keys name unrecoverable secrets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WidgetConfigStore {
    /// Reserved for forward-compatible migrations, mirroring the other stores.
    pub version: u32,
    /// Each placed instance's configuration, keyed by launcher instance id.
    pub instances: HashMap<String, WidgetInstanceConfig>,
}

impl Default for WidgetConfigStore {
    fn default() -> Self {
        Self {
            version: 1,
            instances: HashMap::new(),
        }
    }
}

/// The file widget preferences persist to, alongside `snapshots.json` and
/// `shared-config.json` in the app config directory.
const FILE_NAME: &str = "widgets.json";

impl WidgetConfigStore {
    /// Read the persisted preferences. A missing or unparseable file reads as
    /// the empty store — see the type docs for why that is the intended
    /// "needs configuration" path rather than a recovery.
    pub fn load(dir: &Path) -> Self {
        match Self::load_state(dir) {
            WidgetConfigLoad::Loaded(store) => store,
            WidgetConfigLoad::Absent | WidgetConfigLoad::Corrupt => Self::default(),
        }
    }

    /// Read the persisted preferences, telling an **absent** file (a first run
    /// with no widget placed yet) apart from a **corrupt** one (a file that
    /// exists but cannot be read or parsed). Both still resolve to "needs
    /// configuration" when a specific instance is projected — an absent file has
    /// no entry for the instance, and a corrupt file is explicitly refused — but
    /// the reader learns which it hit so a corrupt file routes *every* instance
    /// to needs-configuration rather than only the ones that happen to be
    /// missing. The bytes are never recovered; a corrupt file is reported as
    /// [`WidgetConfigLoad::Corrupt`], not parsed leniently.
    pub fn load_state(dir: &Path) -> WidgetConfigLoad {
        match std::fs::read_to_string(dir.join(FILE_NAME)) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(store) => WidgetConfigLoad::Loaded(store),
                Err(_) => WidgetConfigLoad::Corrupt,
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => WidgetConfigLoad::Absent,
            Err(_) => WidgetConfigLoad::Corrupt,
        }
    }

    /// Write the preferences atomically (temp-then-rename), the same discipline
    /// every store in this crate uses so a launcher reading the file
    /// concurrently never sees a torn document.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self).expect("widget store serializes");
        let path = dir.join(FILE_NAME);
        let tmp = dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }

    /// The configuration for one instance, or `None` when the launcher has an
    /// instance we have no saved preference for — a fresh placement, or one
    /// whose entry was lost with a corrupt file. Both project as
    /// [`WidgetState::NeedsConfiguration`].
    pub fn get(&self, instance_id: &str) -> Option<&WidgetInstanceConfig> {
        self.instances.get(instance_id)
    }

    /// Record (or replace) one instance's configuration.
    pub fn set(&mut self, instance_id: impl Into<String>, config: WidgetInstanceConfig) {
        self.instances.insert(instance_id.into(), config);
    }

    /// Forget an instance the launcher has removed.
    pub fn remove(&mut self, instance_id: &str) -> Option<WidgetInstanceConfig> {
        self.instances.remove(instance_id)
    }
}

/// The outcome of [`WidgetConfigStore::load_state`]: the readable preferences,
/// or which "empty store" shape was on disk. A corrupt file loses every placed
/// instance's saved selection, so the widget read path routes it to
/// needs-configuration wholesale rather than treating only the missing entry as
/// unconfigured.
#[derive(Debug, Clone, PartialEq)]
pub enum WidgetConfigLoad {
    /// No preference file yet — a first run with nothing placed. An instance the
    /// launcher asks about simply has no entry (needs configuration).
    Absent,
    /// A preference file exists but could not be read or parsed. Discarded, never
    /// recovered; the widget routes every instance to needs-configuration.
    Corrupt,
    /// A readable, parseable preference store.
    Loaded(WidgetConfigStore),
}

/// The fully-resolved projection a launcher renders. Carries the chosen
/// [`WidgetSize`] even for the placeholder states, so the host always knows how
/// large a box it is drawing into.
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetProjection {
    pub size: WidgetSize,
    pub state: WidgetState,
}

/// What a widget shows: real content, or one of the three honest placeholders.
#[derive(Debug, Clone, PartialEq)]
pub enum WidgetState {
    /// The instance has no saved (or no readable) configuration — "Widget needs
    /// configuration". Never a substituted default account.
    NeedsConfiguration,
    /// The instance is configured, but the read model has never been written —
    /// "No data—tap to refresh".
    NoData,
    /// The instance is configured and there is data to show. Boxed because it
    /// is far larger than the two placeholder variants, which would otherwise
    /// bloat every `WidgetProjection` to the content's size.
    Content(Box<WidgetContent>),
}

/// The rendered content of a configured widget with data behind it.
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetContent {
    /// The shared aggregate the widget colours itself from — taken verbatim
    /// from the persisted read model, never recomputed over the selected
    /// subset. This is what keeps the widget colour agreeing with every other
    /// compact surface instead of inventing a subset-only interpretation.
    pub aggregate: AggregateStatus,
    /// How old the whole read model is, for the "as of …" caption. `None` only
    /// before any refresh, which the projection already routes to
    /// [`WidgetState::NoData`], so `Content` carries `Some` in practice.
    pub data_age: Option<Duration>,
    /// The instant `data_age` is measured from — the persisted read model's
    /// own refresh stamp, carried verbatim so a host can render the absolute
    /// "Updated <datetime>" the relative age answers less well (#195). The
    /// same single clock reading as [`WidgetContent::data_age`], never a
    /// second one.
    pub refreshed_at: Option<DateTime<Utc>>,
    /// Whether privacy mode redacted the figures below.
    pub privacy: bool,
    /// The single worst selected headline — the small tier's whole body,
    /// rendered beside the aggregate and the data age. `None` when no selected
    /// account has a readable headline (every one removed, or none selected).
    pub worst: Option<WorstHeadline>,
    /// One row per selected account, in selection order. Empty on the small
    /// tier, which collapses to [`worst`](WidgetContent::worst).
    pub rows: Vec<WidgetRow>,
}

/// One selected account's row.
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetRow {
    /// The account this row deep-links into (or asks to configure, when
    /// removed). Always present so a tap is never ambiguous.
    pub provider_id: String,
    /// The account's display name, retained even under privacy mode.
    pub name: String,
    pub state: RowState,
}

/// A row's content: the account is still present, or it has been removed.
#[derive(Debug, Clone, PartialEq)]
pub enum RowState {
    /// The selected account is gone from the read model — "Account removed—tap
    /// to configure". The widget shows this state explicitly rather than
    /// substituting a different account.
    Removed,
    /// The account is present, with its per-account status and headline cells.
    Present {
        /// The account's own status colour cue.
        status: Status,
        /// The selected headlines, one cell each. Empty when the account's
        /// selection is deliberately empty.
        cells: Vec<HeadlineCell>,
    },
}

/// The worst selected headline, for the small tier and any "headline" glance.
#[derive(Debug, Clone, PartialEq)]
pub struct WorstHeadline {
    /// The account the worst headline belongs to.
    pub name: String,
    /// That account's status (retained under privacy — colours never redact).
    pub status: Status,
    /// The headline cell itself, already redacted if privacy mode is on.
    pub cell: HeadlineCell,
}

/// One headline shown for an account.
#[derive(Debug, Clone, PartialEq)]
pub struct HeadlineCell {
    /// The headline's label, e.g. a window's name or "Balance". Kept under
    /// privacy mode — a name is not a figure.
    pub label: String,
    /// The quota figure, or `None` when privacy mode redacts it.
    pub value: Option<HeadlineValue>,
    /// The usage bar fraction in `0.0..=1.0`. `Some` only on the large tier,
    /// and only for a percentage headline that privacy mode has not redacted.
    pub bar: Option<f64>,
    /// When this window resets, for the large tier's reset information. A reset
    /// time is not a figure, so privacy mode keeps it; it is gated on the large
    /// tier only because that is where there is room to show it.
    pub resets_at: Option<DateTime<Utc>>,
    /// How far through the window's period we are, `0.0..=1.0` — the desktop's
    /// period-progress marker (`src/lib/period.js`), computed here rather than
    /// in the host so there is exactly one interpretation of it (ADR-0006). It
    /// rides on the usage bar, so it is gated to the large tier and redacted
    /// with it under privacy mode; `None` also when the provider reports no
    /// period bounds or the span is non-positive. Drawn against the bar so a
    /// half-full bar at the quarter mark reads as "burning it fast".
    pub period: Option<f64>,
}

/// The figure behind a [`HeadlineCell`], present only when not redacted.
#[derive(Debug, Clone, PartialEq)]
pub enum HeadlineValue {
    /// A usage window: percent used, with the exact allowance when the provider
    /// reports one.
    Usage {
        used_pct: f64,
        allowance: Option<Allowance>,
    },
    /// A credit balance for a pay-per-use account.
    Balance(Credits),
}

/// Project one widget instance into what its launcher should render.
///
/// `instance` is `None` when the launcher knows of a widget we have no readable
/// configuration for (a fresh placement, or one lost to a corrupt preference
/// file); that resolves to [`WidgetState::NeedsConfiguration`]. Otherwise the
/// projection reads the persisted `store` — never fetching — and folds the
/// instance's selection, chosen `size` and privacy mode into content, resolving
/// each removed account and each redacted figure along the way. `now` ages the
/// read model for the "as of …" caption.
pub fn project(
    instance: Option<&WidgetInstanceConfig>,
    store: &SnapshotStore,
    cfg: &Config,
    size: WidgetSize,
    now: DateTime<Utc>,
) -> WidgetProjection {
    let Some(instance) = instance else {
        return WidgetProjection {
            size,
            state: WidgetState::NeedsConfiguration,
        };
    };

    // A configured instance with an empty read model has nothing to show yet —
    // distinct from a removed account (a per-row state) and from an unconfigured
    // instance (no selection at all). "No data—tap to refresh".
    if store.refreshed_at.is_none() || store.snapshots.is_empty() {
        return WidgetProjection {
            size,
            state: WidgetState::NoData,
        };
    }

    let by_id: HashMap<&str, &UsageSnapshot> = store
        .snapshots
        .iter()
        .map(|s| (s.provider_id.as_str(), s))
        .collect();

    let mut rows = Vec::with_capacity(instance.accounts.len());
    for sel in &instance.accounts {
        let snapshot = by_id.get(sel.provider_id.as_str()).copied();
        let name = account_name(cfg, &sel.provider_id, snapshot);
        let state = match snapshot {
            // Not in the read model: the account was removed (or disabled out of
            // it). Explicit, never silently replaced.
            None => RowState::Removed,
            Some(snapshot) => {
                let status = row_status(cfg, snapshot);
                let cells = headline_cells(cfg, snapshot, sel, size, instance.privacy, now);
                RowState::Present { status, cells }
            }
        };
        rows.push(WidgetRow {
            provider_id: sel.provider_id.clone(),
            name,
            state,
        });
    }

    let worst = worst_headline(&rows);

    WidgetProjection {
        size,
        state: WidgetState::Content(Box::new(WidgetContent {
            // The shared aggregate, verbatim — not a subset re-fold.
            aggregate: store.aggregate,
            data_age: store.age(now),
            // The same instant the age above is derived from — the store's own
            // refresh stamp, read once (#195).
            refreshed_at: store.refreshed_at,
            privacy: instance.privacy,
            worst,
            // The small tier collapses to `worst`; only medium/large lay out
            // the per-account rows.
            rows: if size.shows_rows() { rows } else { Vec::new() },
        })),
    }
}

/// The display name for an account: its configured label, else the name the
/// snapshot carried, else the raw id (all a removed account can offer).
fn account_name(cfg: &Config, id: &str, snapshot: Option<&UsageSnapshot>) -> String {
    if let Some(label) = cfg.providers.get(id).and_then(|p| p.label.clone()) {
        return label;
    }
    if let Some(s) = snapshot {
        if !s.provider_name.is_empty() {
            return s.provider_name.clone();
        }
    }
    id.to_string()
}

/// An account's own status cue: the canonical per-account status the rest of
/// the app uses, under this account's effective thresholds. A failed fetch is
/// `Stale`, exactly as every other surface reads it — the widget adds no new
/// interpretation.
fn row_status(cfg: &Config, snapshot: &UsageSnapshot) -> Status {
    let thresholds = cfg.effective_thresholds(&snapshot.provider_id);
    let low = cfg
        .providers
        .get(&snapshot.provider_id)
        .and_then(|p| p.low_balance_warn);
    snapshot.status(thresholds.warn_pct, thresholds.critical_pct, low)
}

/// Build the headline cells for one selected account, honouring the instance's
/// per-account override (or the inherited shared selection), the tier, and
/// privacy mode.
fn headline_cells(
    cfg: &Config,
    snapshot: &UsageSnapshot,
    sel: &WidgetAccountSelection,
    size: WidgetSize,
    privacy: bool,
    now: DateTime<Utc>,
) -> Vec<HeadlineCell> {
    // The instance's own headlines win; otherwise inherit the shared
    // compact-summary selection. `None` at both levels means "decide
    // automatically"; `Some([])` means a deliberately empty selection.
    let metrics = match &sel.headlines {
        Some(list) => Some(list.clone()),
        None => cfg.resolved_mini_metrics(&sel.provider_id),
    };
    // The account's usage schedule (ADR-0007) reshapes only the weekly
    // window's period marker; it is read from the account's own config, not
    // the widget instance, exactly like the desktop surfaces read it.
    let schedule = cfg
        .providers
        .get(&sel.provider_id)
        .map(|p| &p.usage_schedule);
    let large = size.shows_bars_and_reset();
    match metrics {
        Some(list) => list
            .iter()
            .filter_map(|metric| cell_for_metric(snapshot, metric, large, privacy, now, schedule))
            .collect(),
        None => automatic_cells(snapshot, large, privacy, now, schedule),
    }
}

/// One selected headline's cell, or `None` when the metric no longer names
/// anything in the snapshot (a window a provider dropped, or credits on an
/// account that reports none) — the same "yields nothing rather than a
/// misleading calm 0%" rule the tray fold uses.
fn cell_for_metric(
    snapshot: &UsageSnapshot,
    metric: &str,
    large: bool,
    privacy: bool,
    now: DateTime<Utc>,
    schedule: Option<&UsageSchedule>,
) -> Option<HeadlineCell> {
    if let Some(metric_id) = metric.strip_prefix("window:") {
        let window = snapshot.windows.iter().find(|w| w.metric_id == metric_id)?;
        return Some(usage_cell(window, large, privacy, now, schedule));
    }
    if metric == "credits" {
        let credits = snapshot.credits.clone()?;
        return Some(balance_cell(credits, privacy));
    }
    None
}

/// The automatic headline when no selection is pinned: the worst real usage
/// window, or a bare balance if the account is credits-only, mirroring the
/// compact summary's automatic pick.
fn automatic_cells(
    snapshot: &UsageSnapshot,
    large: bool,
    privacy: bool,
    now: DateTime<Utc>,
    schedule: Option<&UsageSchedule>,
) -> Vec<HeadlineCell> {
    let worst = snapshot
        .windows
        .iter()
        .filter(|w| !w.informational)
        .max_by(|a, b| a.used_pct.total_cmp(&b.used_pct));
    if let Some(window) = worst {
        return vec![usage_cell(window, large, privacy, now, schedule)];
    }
    if let Some(credits) = snapshot.credits.clone() {
        return vec![balance_cell(credits, privacy)];
    }
    Vec::new()
}

/// A percentage-usage cell, redacted and tier-gated.
fn usage_cell(
    window: &UsageWindow,
    large: bool,
    privacy: bool,
    now: DateTime<Utc>,
    schedule: Option<&UsageSchedule>,
) -> HeadlineCell {
    HeadlineCell {
        label: window.label.clone(),
        // Privacy hides the figure itself; the label and status stay.
        value: if privacy {
            None
        } else {
            Some(HeadlineValue::Usage {
                used_pct: window.used_pct,
                allowance: window.allowance.clone(),
            })
        },
        // A bar visualises the figure, so it is redacted with the figure and
        // shown only where there is room (the large tier). Clamped: an overage
        // reading above 100% cannot fill past a full bar.
        bar: if large && !privacy {
            Some((window.used_pct / 100.0).clamp(0.0, 1.0))
        } else {
            None
        },
        // A reset time is not a figure — privacy keeps it — but it only appears
        // on the large tier, the one with room for reset information.
        resets_at: if large { window.resets_at } else { None },
        // The period marker is drawn on the bar, so it shares the bar's gate:
        // no bar (below the large tier, or redacted), nothing to mark.
        period: if large && !privacy {
            period_progress(window, now, schedule, &Local)
        } else {
            None
        },
    }
}

/// A credit-balance cell, redacted. Balances carry no percentage, so no bar and
/// no reset time regardless of tier.
fn balance_cell(credits: Credits, privacy: bool) -> HeadlineCell {
    let label = credits
        .label
        .clone()
        .unwrap_or_else(|| "Balance".to_string());
    HeadlineCell {
        label,
        value: if privacy {
            None
        } else {
            Some(HeadlineValue::Balance(credits))
        },
        bar: None,
        resets_at: None,
        period: None,
    }
}

/// The stable metric identity of a weekly-resetting window — the same identity
/// `providers/claude.rs` maps `seven_day` to and the desktop `period.js` keys
/// on. Per-model weekly windows (`weekly_opus`, …) are deliberately not
/// scheduled: the schedule reshapes only the headline weekly window.
const WEEKLY_METRIC_ID: &str = "weekly";

/// How far through the window's period we are, `0.0..=1.0`, or `None` when the
/// provider couldn't tell us the period's bounds. This is the Rust port of the
/// desktop's `periodProgress` (`src/lib/period.js`) — the widget host cannot
/// make quota decisions (ADR-0006) and the JS module is web-only, so the
/// semantics live twice, pinned together by the parity test in this module.
/// Desktop behaviour is untouched; `period.js` remains authoritative for it.
///
/// `schedule` is the account's [`UsageSchedule`] (ADR-0007) and reshapes only a
/// weekly window: the marker then measures scheduled time — the active-day span
/// elapsed so far over the total active-day span in the period — so it advances
/// only on scheduled days and holds flat on off-days. Every other window, and a
/// weekly window whose schedule is all-seven, zero-day or absent, keeps the raw
/// calendar fraction.
///
/// `tz` supplies the local calendar the day boundaries are walked in —
/// production passes the device's [`Local`], matching the desktop's
/// device-local semantics; tests pass `Utc` for determinism. Day boundaries are
/// local midnights, advanced by calendar date rather than +24h so a DST shift
/// doesn't move them.
fn period_progress<T: TimeZone>(
    window: &UsageWindow,
    now: DateTime<Utc>,
    schedule: Option<&UsageSchedule>,
    tz: &T,
) -> Option<f64> {
    let start = window.period_start?;
    let end = window.resets_at?;
    let span_ms = (end - start).num_milliseconds() as f64;
    // `<= 0` is `period.js`'s `!(span > 0)`: a degenerate span is garbage, not
    // "already reset". (The span is an integer-ms difference, never NaN.)
    if span_ms <= 0.0 {
        return None;
    }
    let calendar = ((now - start).num_milliseconds() as f64 / span_ms).clamp(0.0, 1.0);

    // Only the weekly window is reshaped, and only by a partial schedule: an
    // absent, all-seven or zero-day schedule all mean "pace against the raw
    // calendar".
    let active = if window.metric_id == WEEKLY_METRIC_ID {
        schedule.and_then(active_weekdays)
    } else {
        None
    };
    let Some(active) = active else {
        return Some(calendar);
    };

    // Sum the active-day spans inside [start, end], and inside [start, now],
    // walking the local-midnight day boundaries so an off-day contributes
    // nothing and a boundary day counts only the hours that actually fall in
    // the period.
    let mut total_ms = 0.0f64;
    let mut elapsed_ms = 0.0f64;
    let now_capped = now.min(end);
    let mut day = start.with_timezone(tz).date_naive();
    loop {
        let midnight = midnight_instant(tz, day)?;
        if midnight >= end {
            break;
        }
        let day_end = midnight_instant(tz, day.succ_opt()?)?;
        if active[weekday_index(day)] {
            let from = midnight.max(start);
            total_ms += (day_end.min(end) - from).num_milliseconds() as f64;
            elapsed_ms += (day_end.min(now_capped) - from).num_milliseconds().max(0) as f64;
        }
        day = day.succ_opt()?;
    }
    if total_ms <= 0.0 {
        return Some(calendar);
    }
    Some((elapsed_ms / total_ms).clamp(0.0, 1.0))
}

/// The active weekdays a schedule names, indexed by
/// [`weekday_index`] (Sunday 0 … Saturday 6, matching JS `getDay()`), or `None`
/// when there is no schedule to apply: an absent schedule, every day active, or
/// zero days active all mean "pace against the raw calendar".
fn active_weekdays(schedule: &UsageSchedule) -> Option<[bool; 7]> {
    let days = [
        schedule.sunday,
        schedule.monday,
        schedule.tuesday,
        schedule.wednesday,
        schedule.thursday,
        schedule.friday,
        schedule.saturday,
    ];
    let active = days.iter().filter(|day| **day).count();
    if active == 0 || active == 7 {
        None
    } else {
        Some(days)
    }
}

/// JS `getDay()`'s 0=Sunday…6=Saturday for a calendar day.
fn weekday_index(day: NaiveDate) -> usize {
    day.weekday().num_days_from_sunday() as usize
}

/// The instant of local midnight on `day`. Where a transition lands exactly on
/// midnight, JS's `setHours(0, 0, 0, 0)` resolves an ambiguous (repeated) hour
/// to the earlier reading and a skipped midnight to an hour inside the same
/// calendar day — this mirrors both so the day walk agrees with the desktop.
fn midnight_instant<T: TimeZone>(tz: &T, day: NaiveDate) -> Option<DateTime<Utc>> {
    let naive = day.and_hms_opt(0, 0, 0)?;
    let resolved = match tz.from_local_datetime(&naive) {
        LocalResult::None => tz.from_local_datetime(&(naive + Duration::hours(1))),
        resolved => resolved,
    };
    resolved.earliest().map(|dt| dt.with_timezone(&Utc))
}

/// The worst selected headline across the present rows: the row with the worst
/// status, breaking ties by the loudest percentage, then that row's own
/// worst-percentage cell. Removed rows and rows with no cells cannot be the
/// worst headline — there is nothing to show for them. Status is compared even
/// under privacy (colours never redact); the percentage tie-break reads the
/// figure internally without ever displaying it.
fn worst_headline(rows: &[WidgetRow]) -> Option<WorstHeadline> {
    let mut best: Option<(Status, f64, &WidgetRow, &HeadlineCell)> = None;
    for row in rows {
        let RowState::Present { status, cells } = &row.state else {
            continue;
        };
        let Some(cell) = cells
            .iter()
            .max_by(|a, b| cell_pct(a).total_cmp(&cell_pct(b)))
        else {
            continue;
        };
        let pct = cell_pct(cell);
        let better = match best {
            None => true,
            Some((best_status, best_pct, _, _)) => (*status, pct) > (best_status, best_pct),
        };
        if better {
            best = Some((*status, pct, row, cell));
        }
    }
    best.map(|(status, _, row, cell)| WorstHeadline {
        name: row.name.clone(),
        status,
        cell: cell.clone(),
    })
}

/// A cell's percentage for ranking, or `-inf` when it carries none (a balance,
/// or a privacy-redacted cell) so it never outranks a real percentage. Reads
/// the usage figure for ordering only — the redaction of `value` still governs
/// what is *displayed*.
fn cell_pct(cell: &HeadlineCell) -> f64 {
    match &cell.value {
        Some(HeadlineValue::Usage { used_pct, .. }) => *used_pct,
        _ => f64::NEG_INFINITY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use crate::model::{FetchError, UsageWindow};

    fn window(metric_id: &str, label: &str, pct: f64) -> UsageWindow {
        UsageWindow {
            metric_id: metric_id.into(),
            label: label.into(),
            used_pct: pct,
            resets_at: Some(Utc::now() + Duration::hours(3)),
            ..Default::default()
        }
    }

    fn snap(id: &str, windows: Vec<UsageWindow>) -> UsageSnapshot {
        UsageSnapshot::ok(id, id, windows, None)
    }

    /// A read model that has definitely been refreshed, carrying `snapshots`
    /// in the given order and an aggregate a caller can override.
    fn store_with(snapshots: Vec<UsageSnapshot>, aggregate: AggregateStatus) -> SnapshotStore {
        // `from_snapshots` stamps `refreshed_at`, which is what distinguishes a
        // populated read model from the empty "no data" one.
        SnapshotStore::from_snapshots(snapshots, aggregate)
    }

    fn cfg_with(enabled: &[&str]) -> Config {
        let mut cfg = Config::default();
        cfg.providers.clear();
        for id in enabled {
            cfg.providers.insert(
                (*id).into(),
                ProviderConfig {
                    enabled: true,
                    ..Default::default()
                },
            );
        }
        cfg
    }

    fn instance(ids: &[&str]) -> WidgetInstanceConfig {
        WidgetInstanceConfig {
            accounts: ids
                .iter()
                .map(|id| WidgetAccountSelection {
                    provider_id: (*id).into(),
                    headlines: None,
                })
                .collect(),
            privacy: false,
        }
    }

    fn present(projection: &WidgetProjection) -> &WidgetContent {
        match &projection.state {
            WidgetState::Content(c) => c,
            other => panic!("expected content, got {other:?}"),
        }
    }

    // ---- Breakpoint selection -------------------------------------------

    /// Acceptance: small/medium/large chosen from launcher dimensions. Both
    /// axes must clear a tier's floor, so a box that is wide but short — or tall
    /// but narrow — steps down instead of cropping a richer tier.
    #[test]
    fn breakpoints_pick_the_richest_tier_that_fits_both_axes() {
        assert_eq!(WidgetSize::from_dimensions(120.0, 80.0), WidgetSize::Small);
        assert_eq!(
            WidgetSize::from_dimensions(200.0, 140.0),
            WidgetSize::Medium
        );
        assert_eq!(WidgetSize::from_dimensions(300.0, 260.0), WidgetSize::Large);

        // Wide but short, and tall but narrow: neither reaches the tier its
        // larger dimension alone might suggest.
        assert_eq!(WidgetSize::from_dimensions(400.0, 90.0), WidgetSize::Small);
        assert_eq!(
            WidgetSize::from_dimensions(300.0, 150.0),
            WidgetSize::Medium
        );
        assert_eq!(
            WidgetSize::from_dimensions(200.0, 400.0),
            WidgetSize::Medium
        );

        // Exactly on a floor counts as fitting it.
        assert_eq!(WidgetSize::from_dimensions(250.0, 200.0), WidgetSize::Large);
        // A hair under the large height drops to medium, not a cropped large.
        assert_eq!(
            WidgetSize::from_dimensions(250.0, 199.0),
            WidgetSize::Medium
        );
    }

    /// A launcher can report a non-finite/degenerate dimension mid-resize; the
    /// widget degrades to the smallest tier rather than picking off garbage.
    #[test]
    fn degenerate_dimensions_degrade_to_small() {
        assert_eq!(
            WidgetSize::from_dimensions(f64::NAN, 260.0),
            WidgetSize::Small
        );
        assert_eq!(
            WidgetSize::from_dimensions(300.0, f64::INFINITY),
            WidgetSize::Small
        );
        assert_eq!(WidgetSize::from_dimensions(-10.0, -10.0), WidgetSize::Small);
    }

    // ---- Placeholder states ---------------------------------------------

    /// Acceptance: corrupt/missing prefs → "Widget needs configuration". No
    /// instance config projects to exactly that, whatever the read model holds.
    #[test]
    fn a_missing_instance_config_needs_configuration() {
        let store = store_with(
            vec![snap("a", vec![window("w", "W", 10.0)])],
            AggregateStatus::default(),
        );
        let cfg = cfg_with(&["a"]);
        let p = project(None, &store, &cfg, WidgetSize::Large, Utc::now());
        assert_eq!(p.state, WidgetState::NeedsConfiguration);
        // Size is still resolved so the host can size the placeholder.
        assert_eq!(p.size, WidgetSize::Large);
    }

    /// Acceptance: no snapshot → "No data—tap to refresh". A configured
    /// instance over an empty read model shows the empty state, not removed
    /// rows and not invented data.
    #[test]
    fn a_configured_instance_over_an_empty_read_model_is_no_data() {
        let cfg = cfg_with(&["a"]);
        let inst = instance(&["a"]);
        // The default store has never been refreshed.
        let p = project(
            Some(&inst),
            &SnapshotStore::default(),
            &cfg,
            WidgetSize::Medium,
            Utc::now(),
        );
        assert_eq!(p.state, WidgetState::NoData);
    }

    // ---- Removed-account state ------------------------------------------

    /// Acceptance: a removed account becomes an explicit removed row, and no
    /// other account is silently shown in its place.
    #[test]
    fn a_selected_account_absent_from_the_read_model_is_removed_not_substituted() {
        // The read model has only "b"; the instance selected "a" and "b".
        let store = store_with(
            vec![snap("b", vec![window("w", "W", 50.0)])],
            AggregateStatus::default(),
        );
        let cfg = cfg_with(&["b"]); // "a" was deleted from config too.
        let inst = instance(&["a", "b"]);
        let content = {
            let p = project(Some(&inst), &store, &cfg, WidgetSize::Medium, Utc::now());
            present(&p).clone()
        };

        assert_eq!(content.rows.len(), 2, "both selections keep their row");
        let a = &content.rows[0];
        assert_eq!(a.provider_id, "a");
        assert_eq!(a.state, RowState::Removed);
        // No silent substitution: the removed row still deep-links to "a", and
        // "b" appears only in its own row.
        assert!(matches!(content.rows[1].state, RowState::Present { .. }));
        assert_eq!(content.rows[1].provider_id, "b");
    }

    // ---- Tiered content --------------------------------------------------

    /// The small tier collapses to the worst headline: no rows, but a worst
    /// headline and the shared aggregate/age are still there to render.
    #[test]
    fn the_small_tier_collapses_to_the_worst_headline() {
        let store = store_with(
            vec![
                snap("calm", vec![window("w", "Calm", 10.0)]),
                snap("busy", vec![window("w", "Busy", 95.0)]),
            ],
            AggregateStatus {
                status: Status::Critical,
                pct: 95.0,
            },
        );
        let cfg = cfg_with(&["calm", "busy"]);
        let inst = instance(&["calm", "busy"]);
        let p = project(Some(&inst), &store, &cfg, WidgetSize::Small, Utc::now());
        let content = present(&p);

        assert!(content.rows.is_empty(), "small tier lays out no rows");
        let worst = content.worst.as_ref().expect("a worst headline");
        assert_eq!(worst.name, "busy", "the loudest account is the worst");
        assert_eq!(worst.status, Status::Critical);
    }

    /// The medium tier shows per-account rows with values and status, but no
    /// usage bar and no reset information — those are the large tier's.
    #[test]
    fn the_medium_tier_shows_rows_with_values_but_no_bars_or_reset() {
        let store = store_with(
            vec![snap("a", vec![window("w", "W", 42.0)])],
            AggregateStatus::default(),
        );
        let cfg = cfg_with(&["a"]);
        let inst = instance(&["a"]);
        let p = project(Some(&inst), &store, &cfg, WidgetSize::Medium, Utc::now());
        let content = present(&p);

        assert_eq!(content.rows.len(), 1);
        let RowState::Present { cells, .. } = &content.rows[0].state else {
            panic!("present row");
        };
        let cell = &cells[0];
        assert!(
            matches!(cell.value, Some(HeadlineValue::Usage { used_pct, .. }) if used_pct == 42.0)
        );
        assert!(cell.bar.is_none(), "no bar below the large tier");
        assert!(
            cell.resets_at.is_none(),
            "no reset info below the large tier"
        );
    }

    /// The large tier adds the usage bar and reset information.
    #[test]
    fn the_large_tier_adds_the_usage_bar_and_reset_information() {
        let store = store_with(
            vec![snap("a", vec![window("w", "W", 42.0)])],
            AggregateStatus::default(),
        );
        let cfg = cfg_with(&["a"]);
        let inst = instance(&["a"]);
        let p = project(Some(&inst), &store, &cfg, WidgetSize::Large, Utc::now());
        let RowState::Present { cells, .. } = &present(&p).rows[0].state else {
            panic!("present row");
        };
        let cell = &cells[0];
        assert_eq!(cell.bar, Some(0.42));
        assert!(cell.resets_at.is_some(), "reset info on the large tier");
    }

    /// An overage reading above 100% cannot fill the bar past full.
    #[test]
    fn the_usage_bar_clamps_overage_to_full() {
        let store = store_with(
            vec![snap("a", vec![window("w", "W", 150.0)])],
            AggregateStatus::default(),
        );
        let cfg = cfg_with(&["a"]);
        let inst = instance(&["a"]);
        let p = project(Some(&inst), &store, &cfg, WidgetSize::Large, Utc::now());
        let RowState::Present { cells, .. } = &present(&p).rows[0].state else {
            panic!("present row");
        };
        assert_eq!(cells[0].bar, Some(1.0));
    }

    // ---- Privacy redaction ----------------------------------------------

    /// Acceptance: privacy mode hides figures/balances while keeping names and
    /// status colours. Reset info (a time, not a figure) stays; the bar (a
    /// figure) goes.
    #[test]
    fn privacy_mode_redacts_figures_but_keeps_names_status_and_reset() {
        let store = store_with(
            vec![snap("a", vec![window("w", "Rolling window", 42.0)])],
            AggregateStatus::default(),
        );
        let cfg = cfg_with(&["a"]);
        let inst = WidgetInstanceConfig {
            privacy: true,
            ..instance(&["a"])
        };
        let p = project(Some(&inst), &store, &cfg, WidgetSize::Large, Utc::now());
        let content = present(&p);
        assert!(content.privacy);

        let row = &content.rows[0];
        assert_eq!(row.name, "a", "the account name survives redaction");
        let RowState::Present { status, cells } = &row.state else {
            panic!("present row");
        };
        assert_eq!(*status, Status::Ok, "status colour is retained");
        let cell = &cells[0];
        assert_eq!(cell.label, "Rolling window", "the headline label survives");
        assert!(cell.value.is_none(), "the figure is redacted");
        assert!(
            cell.bar.is_none(),
            "the bar (a figure) is redacted even on large"
        );
        assert!(
            cell.resets_at.is_some(),
            "the reset time (not a figure) survives"
        );
    }

    /// A credit balance is a figure too, redacted under privacy.
    #[test]
    fn privacy_mode_redacts_a_credit_balance() {
        let credits = Credits {
            balance: 12.5,
            label: Some("Wallet".into()),
            unit: "USD".into(),
            used: None,
            granted: None,
            est_tokens_remaining: None,
        };
        let s = UsageSnapshot::ok("a", "a", vec![], Some(credits));
        let store = store_with(vec![s], AggregateStatus::default());
        let cfg = cfg_with(&["a"]);
        let inst = WidgetInstanceConfig {
            privacy: true,
            ..instance(&["a"])
        };
        let p = project(Some(&inst), &store, &cfg, WidgetSize::Large, Utc::now());
        let RowState::Present { cells, .. } = &present(&p).rows[0].state else {
            panic!("present row");
        };
        assert_eq!(cells[0].label, "Wallet", "the balance's name survives");
        assert!(cells[0].value.is_none(), "the balance figure is redacted");
    }

    // ---- Aggregate follows the shared status ----------------------------

    /// Acceptance: the widget status colour follows the shared aggregate, not a
    /// new interpretation. Even when every *selected* account is calm, a
    /// Critical shared aggregate colours the widget Critical — it is taken
    /// verbatim from the persisted read model.
    #[test]
    fn the_widget_colour_follows_the_shared_aggregate_not_the_selected_subset() {
        let store = store_with(
            vec![snap("a", vec![window("w", "W", 5.0)])],
            AggregateStatus {
                status: Status::Critical,
                pct: 99.0,
            },
        );
        let cfg = cfg_with(&["a"]);
        let inst = instance(&["a"]);
        let p = project(Some(&inst), &store, &cfg, WidgetSize::Medium, Utc::now());
        let content = present(&p);
        assert_eq!(content.aggregate.status, Status::Critical);
        assert_eq!(content.aggregate.pct, 99.0);
        // The account's own row still reports its honest (calm) per-account
        // status — the aggregate colour is a separate, shared signal.
        let RowState::Present { status, .. } = &content.rows[0].state else {
            panic!("present row");
        };
        assert_eq!(*status, Status::Ok);
    }

    /// Acceptance (#195): the read model's authoritative refresh instant rides
    /// through the projection beside the relative `data_age` — derived from the
    /// *same* single clock reading the age is, never a second one. A widget
    /// showing "Updated <datetime>" must show the instant "as of <age>" is
    /// measured from.
    #[test]
    fn the_projection_carries_the_read_models_refresh_instant() {
        let refreshed = Utc.with_ymd_and_hms(2025, 6, 1, 12, 0, 0).unwrap();
        let now = refreshed + Duration::seconds(90);
        let store = SnapshotStore {
            refreshed_at: Some(refreshed),
            ..store_with(
                vec![snap("a", vec![window("w", "W", 10.0)])],
                AggregateStatus::default(),
            )
        };
        let cfg = cfg_with(&["a"]);
        let inst = instance(&["a"]);
        let p = project(Some(&inst), &store, &cfg, WidgetSize::Medium, now);
        let content = present(&p);

        assert_eq!(
            content.refreshed_at,
            Some(refreshed),
            "the content carries the store's refresh instant verbatim"
        );
        // `data_age` is unchanged: still exactly the store's age at `now`, so
        // refresh + age reconstructs `now` — one instant, two presentations.
        assert_eq!(content.data_age, Some(now - refreshed));
    }

    /// A failed fetch reads as Stale on the row, the same as every other
    /// surface — no widget-specific reinterpretation.
    #[test]
    fn a_failed_account_row_reads_stale() {
        let failed = UsageSnapshot::failed("a", "a", FetchError::Network("boom".into()));
        let store = store_with(
            vec![failed],
            AggregateStatus {
                status: Status::Stale,
                pct: 0.0,
            },
        );
        let cfg = cfg_with(&["a"]);
        let inst = instance(&["a"]);
        let p = project(Some(&inst), &store, &cfg, WidgetSize::Medium, Utc::now());
        let RowState::Present { status, .. } = &present(&p).rows[0].state else {
            panic!("present row");
        };
        assert_eq!(*status, Status::Stale);
    }

    // ---- Per-instance config & inheritance ------------------------------

    /// Acceptance: multiple instances retain independent selections. Two
    /// instances over the same read model, configured differently (privacy,
    /// account set, headline pin), project independently.
    #[test]
    fn two_instances_project_independently() {
        let store = store_with(
            vec![
                snap(
                    "a",
                    vec![window("m1", "One", 10.0), window("m2", "Two", 80.0)],
                ),
                snap("b", vec![window("w", "W", 20.0)]),
            ],
            AggregateStatus::default(),
        );
        let cfg = cfg_with(&["a", "b"]);

        // Instance 1: both accounts, no privacy, automatic headlines.
        let one = instance(&["a", "b"]);
        // Instance 2: only "a", privacy on, headline pinned to the m1 window.
        let two = WidgetInstanceConfig {
            accounts: vec![WidgetAccountSelection {
                provider_id: "a".into(),
                headlines: Some(vec!["window:m1".into()]),
            }],
            privacy: true,
        };

        let p1 = project(Some(&one), &store, &cfg, WidgetSize::Medium, Utc::now());
        let p2 = project(Some(&two), &store, &cfg, WidgetSize::Medium, Utc::now());
        let c1 = present(&p1);
        let c2 = present(&p2);

        assert_eq!(c1.rows.len(), 2, "instance 1 shows both accounts");
        assert!(!c1.privacy);
        // Instance 1's automatic headline for "a" is the worst window (m2, 80%).
        let RowState::Present { cells, .. } = &c1.rows[0].state else {
            panic!("present");
        };
        assert!(
            matches!(&cells[0].value, Some(HeadlineValue::Usage { used_pct, .. }) if *used_pct == 80.0)
        );

        assert_eq!(c2.rows.len(), 1, "instance 2 shows only its one account");
        assert!(c2.privacy);
        // Instance 2 pinned m1, so its cell names m1 — redacted, but labelled.
        let RowState::Present { cells, .. } = &c2.rows[0].state else {
            panic!("present");
        };
        assert_eq!(cells[0].label, "One");
        assert!(cells[0].value.is_none(), "instance 2 redacts");
    }

    /// A per-instance headline override does not disturb the shared config, so
    /// another instance inheriting the shared selection is unaffected.
    #[test]
    fn a_per_instance_override_leaves_the_shared_selection_untouched() {
        let mut cfg = cfg_with(&["a"]);
        cfg.providers.get_mut("a").unwrap().mini_summary_metrics = Some(vec!["window:m1".into()]);
        let store = store_with(
            vec![snap(
                "a",
                vec![window("m1", "One", 10.0), window("m2", "Two", 90.0)],
            )],
            AggregateStatus::default(),
        );

        // This instance overrides to m2.
        let overriding = WidgetInstanceConfig {
            accounts: vec![WidgetAccountSelection {
                provider_id: "a".into(),
                headlines: Some(vec!["window:m2".into()]),
            }],
            privacy: false,
        };
        // This instance inherits the shared selection (m1).
        let inheriting = instance(&["a"]);

        let over = project(
            Some(&overriding),
            &store,
            &cfg,
            WidgetSize::Medium,
            Utc::now(),
        );
        let inherit = project(
            Some(&inheriting),
            &store,
            &cfg,
            WidgetSize::Medium,
            Utc::now(),
        );

        let RowState::Present {
            cells: over_cells, ..
        } = &present(&over).rows[0].state
        else {
            panic!("present");
        };
        let RowState::Present {
            cells: inherit_cells,
            ..
        } = &present(&inherit).rows[0].state
        else {
            panic!("present");
        };
        assert_eq!(over_cells[0].label, "Two", "override took m2");
        assert_eq!(
            inherit_cells[0].label, "One",
            "inheritance kept the shared m1"
        );
    }

    /// `inherit_shared` seeds a new instance from the enabled accounts that are
    /// in the shared compact summary, skipping ones that opted out with an
    /// empty selection, and in configured order.
    #[test]
    fn inherit_shared_seeds_from_the_shared_compact_summary_selection() {
        let mut cfg = cfg_with(&["a", "b", "c"]);
        // "b" opted out of the compact summary entirely.
        cfg.providers.get_mut("b").unwrap().mini_summary_metrics = Some(vec![]);
        // "c" is disabled, so it is not part of the shared surface.
        cfg.providers.get_mut("c").unwrap().enabled = false;

        let seeded = WidgetInstanceConfig::inherit_shared(&cfg);
        let ids: Vec<&str> = seeded
            .accounts
            .iter()
            .map(|a| a.provider_id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["a"],
            "only the enabled, non-opted-out account seeds"
        );
        assert!(
            seeded.accounts.iter().all(|a| a.headlines.is_none()),
            "inherits shared headlines"
        );
        assert!(!seeded.privacy);
    }

    // ---- Store persistence ----------------------------------------------

    /// The preference store round-trips independent instances through disk.
    #[test]
    fn the_widget_store_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = WidgetConfigStore::default();
        store.set("id-1", instance(&["a", "b"]));
        store.set(
            "id-2",
            WidgetInstanceConfig {
                privacy: true,
                ..instance(&["a"])
            },
        );
        store.save(dir.path()).unwrap();

        let loaded = WidgetConfigStore::load(dir.path());
        assert_eq!(loaded, store);
        assert_eq!(loaded.get("id-1"), Some(&instance(&["a", "b"])));
        assert!(loaded.get("id-2").unwrap().privacy);
        assert!(loaded.get("missing").is_none());
    }

    /// A corrupt or missing preference file loads as the empty store, so any
    /// instance the launcher asks about projects "needs configuration" — the
    /// acceptance criterion, exercised through the store + projection seam.
    #[test]
    fn a_corrupt_preference_file_makes_every_instance_need_configuration() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), "{ not json").unwrap();
        let prefs = WidgetConfigStore::load(dir.path());
        assert_eq!(prefs, WidgetConfigStore::default());

        let store = store_with(
            vec![snap("a", vec![window("w", "W", 10.0)])],
            AggregateStatus::default(),
        );
        let cfg = cfg_with(&["a"]);
        // The launcher knows of "placed-instance", but the corrupt file lost it.
        let p = project(
            prefs.get("placed-instance"),
            &store,
            &cfg,
            WidgetSize::Medium,
            Utc::now(),
        );
        assert_eq!(p.state, WidgetState::NeedsConfiguration);
    }

    /// `load_state` tells an absent preference file (first run) apart from a
    /// corrupt one, while `load` still collapses both to the empty store.
    #[test]
    fn load_state_distinguishes_absent_corrupt_and_loaded() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            WidgetConfigStore::load_state(dir.path()),
            WidgetConfigLoad::Absent
        );

        std::fs::write(dir.path().join(FILE_NAME), "{ not json").unwrap();
        assert_eq!(
            WidgetConfigStore::load_state(dir.path()),
            WidgetConfigLoad::Corrupt
        );
        // `load` still discards a corrupt file to the empty store.
        assert_eq!(
            WidgetConfigStore::load(dir.path()),
            WidgetConfigStore::default()
        );

        let mut written = WidgetConfigStore::default();
        written.set("id-1", instance(&["a"]));
        written.save(dir.path()).unwrap();
        assert_eq!(
            WidgetConfigStore::load_state(dir.path()),
            WidgetConfigLoad::Loaded(written)
        );
    }

    /// Removing an instance the launcher deleted forgets exactly that one.
    #[test]
    fn removing_an_instance_forgets_only_that_instance() {
        let mut store = WidgetConfigStore::default();
        store.set("keep", instance(&["a"]));
        store.set("drop", instance(&["b"]));
        assert!(store.remove("drop").is_some());
        assert!(store.get("drop").is_none());
        assert!(store.get("keep").is_some());
    }

    // ---- Period-progress marker (issue #189) -----------------------------

    use crate::config::UsageSchedule;

    /// The fraction the widget's marker sits at, computed against UTC-local
    /// days. Deterministic, so fixtures can be written as plain UTC dates; the
    /// projection-level tests below use [`local_at`] instead, because there the
    /// timezone comes from `chrono::Local` and the fixtures must agree with it.
    fn progress(
        metric_id: &str,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        now: DateTime<Utc>,
        schedule: Option<&UsageSchedule>,
    ) -> Option<f64> {
        period_progress(&bounded(metric_id, start, end), now, schedule, &Utc)
    }

    /// A usage window with explicit period bounds.
    fn bounded(metric_id: &str, start: DateTime<Utc>, end: DateTime<Utc>) -> UsageWindow {
        UsageWindow {
            metric_id: metric_id.into(),
            label: "W".into(),
            used_pct: 40.0,
            resets_at: Some(end),
            period_start: Some(start),
            ..Default::default()
        }
    }

    fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    /// Same wall-clock reading in the process's local zone, as an instant — the
    /// projection tests build fixtures this way so the expected fractions hold
    /// in whatever zone CI or a laptop happens to run in (the same trick
    /// `src/lib/period.test.js` uses).
    fn local_at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Local
            .with_ymd_and_hms(y, m, d, h, 0, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn monfri() -> UsageSchedule {
        UsageSchedule {
            monday: true,
            tuesday: true,
            wednesday: true,
            thursday: true,
            friday: true,
            saturday: false,
            sunday: false,
        }
    }

    fn none_active() -> UsageSchedule {
        UsageSchedule {
            monday: false,
            tuesday: false,
            wednesday: false,
            thursday: false,
            friday: false,
            saturday: false,
            sunday: false,
        }
    }

    // 2025-01-06 is a Monday: the week runs Monday 00:00 → Monday 00:00, the
    // same fixtures `src/lib/period.test.js` pins.
    fn week() -> (DateTime<Utc>, DateTime<Utc>) {
        (at(2025, 1, 6, 0), at(2025, 1, 13, 0))
    }

    /// Missing period bounds mean the provider never told us when the period
    /// began or ends — no marker, not a fabricated 0%.
    #[test]
    fn missing_period_bounds_yield_no_marker() {
        let (start, end) = week();
        let mut w = bounded("weekly", start, end);
        w.period_start = None;
        assert_eq!(period_progress(&w, at(2025, 1, 8, 12), None, &Utc), None);
        let mut w = bounded("weekly", start, end);
        w.resets_at = None;
        assert_eq!(period_progress(&w, at(2025, 1, 8, 12), None, &Utc), None);
    }

    /// A non-positive span (start ≥ end) is garbage, not "already reset" —
    /// `period.js` refuses it and so does the port.
    #[test]
    fn a_non_positive_span_yields_no_marker() {
        assert_eq!(
            progress(
                "weekly",
                at(2025, 1, 6, 0),
                at(2025, 1, 6, 0),
                at(2025, 1, 6, 1),
                None
            ),
            None
        );
        assert_eq!(
            progress(
                "weekly",
                at(2025, 1, 13, 0),
                at(2025, 1, 6, 0),
                at(2025, 1, 8, 0),
                None
            ),
            None
        );
    }

    /// The raw calendar fraction clamps to [0, 1] before the period began and
    /// at/past the reset.
    #[test]
    fn the_calendar_fraction_clamps_at_the_period_bounds() {
        let (start, end) = week();
        assert_eq!(
            progress("weekly", start, end, at(2025, 1, 1, 12), None),
            Some(0.0)
        );
        // Exactly at the reset counts as fully elapsed.
        assert_eq!(progress("weekly", start, end, end, None), Some(1.0));
        assert_eq!(
            progress("weekly", start, end, at(2025, 1, 20, 12), None),
            Some(1.0)
        );
    }

    /// A weekly window with a Mon–Fri schedule paces a fifth of the allowance
    /// per working day: Wednesday noon is halfway through the five working
    /// days, not 2.5/7 through the raw week.
    #[test]
    fn a_weekly_window_paces_a_mon_fri_schedule_across_working_days() {
        let (start, end) = week();
        let monfri = monfri();
        assert_eq!(
            progress("weekly", start, end, at(2025, 1, 8, 12), Some(&monfri)),
            Some(0.5)
        );
        assert_eq!(
            progress("weekly", start, end, at(2025, 1, 6, 12), Some(&monfri)),
            Some(0.1)
        );
        assert_eq!(
            progress("weekly", start, end, at(2025, 1, 7, 12), Some(&monfri)),
            Some(0.3)
        );
    }

    /// Once every working day has elapsed the marker sits at 100% and holds
    /// flat across the off-days instead of creeping towards Monday.
    #[test]
    fn a_scheduled_weekly_marker_freezes_across_off_days() {
        let (start, end) = week();
        let monfri = monfri();
        assert_eq!(
            progress("weekly", start, end, at(2025, 1, 10, 18), Some(&monfri)),
            Some(0.95)
        );
        assert_eq!(
            progress("weekly", start, end, at(2025, 1, 11, 0), Some(&monfri)),
            Some(1.0)
        );
        assert_eq!(
            progress("weekly", start, end, at(2025, 1, 11, 12), Some(&monfri)),
            Some(1.0)
        );
        assert_eq!(
            progress("weekly", start, end, at(2025, 1, 12, 12), Some(&monfri)),
            Some(1.0)
        );
    }

    /// An all-seven schedule, a zero-day one, and an absent one all pace
    /// against the raw calendar — the schedule only matters when it is partial.
    #[test]
    fn an_all_seven_zero_day_or_absent_schedule_uses_the_raw_calendar() {
        let (start, end) = week();
        let now = at(2025, 1, 8, 12);
        let calendar = 2.5 / 7.0;
        assert_eq!(
            progress("weekly", start, end, now, Some(&UsageSchedule::default())),
            Some(calendar)
        );
        assert_eq!(
            progress("weekly", start, end, now, Some(&none_active())),
            Some(calendar)
        );
        assert_eq!(progress("weekly", start, end, now, None), Some(calendar));
    }

    /// Only the headline weekly window is scheduled; a five-hour window and a
    /// monthly cap keep the raw calendar fraction even with a partial schedule.
    #[test]
    fn a_non_weekly_window_ignores_the_schedule() {
        let monfri = monfri();
        let start = at(2025, 1, 6, 8);
        let five_hour_end = start + Duration::hours(5);
        let now = start + Duration::hours(2);
        let calendar = 0.4;
        assert_eq!(
            progress("five_hour", start, five_hour_end, now, Some(&monfri)),
            Some(calendar)
        );
        let monthly_end = start + Duration::days(30);
        assert_eq!(
            progress(
                "monthly_cap",
                start,
                monthly_end,
                start + Duration::days(15),
                Some(&monfri)
            ),
            Some(0.5)
        );
    }

    /// A period that starts mid-week: the boundary days are half-days and the
    /// weekend freezes, so the fraction counts partial boundary days
    /// fractionally (0.25 of a Thursday is 0.05, not a whole fifth).
    #[test]
    fn a_mid_week_period_counts_partial_boundary_days_fractionally() {
        let monfri = monfri();
        let start = at(2025, 1, 9, 12); // Thursday noon
        let end = at(2025, 1, 16, 12); // Thursday noon, weekend mid-period
        assert_eq!(
            progress("weekly", start, end, at(2025, 1, 10, 12), Some(&monfri)),
            Some(0.2)
        );
        assert_eq!(
            progress("weekly", start, end, at(2025, 1, 11, 0), Some(&monfri)),
            Some(0.3)
        );
        assert_eq!(
            progress("weekly", start, end, at(2025, 1, 12, 12), Some(&monfri)),
            Some(0.3)
        );
        assert_eq!(
            progress("weekly", start, end, at(2025, 1, 13, 12), Some(&monfri)),
            Some(0.4)
        );
        // Six hours into the half-day boundary Thursday: 0.25 of five working
        // days, not a whole day (which would read 0.2).
        assert_eq!(
            progress("weekly", start, end, at(2025, 1, 9, 18), Some(&monfri)),
            Some(0.05)
        );
    }

    /// Acceptance: on the large tier, a window with `period_start` +
    /// `resets_at` renders a marker; the projection carries the fraction.
    #[test]
    fn the_large_tier_carries_a_period_fraction() {
        let start = local_at(2025, 1, 6, 0);
        let end = local_at(2025, 1, 13, 0);
        let now = local_at(2025, 1, 8, 12);
        let store = store_with(
            vec![snap("a", vec![bounded("weekly", start, end)])],
            AggregateStatus::default(),
        );
        let cfg = cfg_with(&["a"]);
        let inst = instance(&["a"]);
        let p = project(Some(&inst), &store, &cfg, WidgetSize::Large, now);
        let RowState::Present { cells, .. } = &present(&p).rows[0].state else {
            panic!("present row");
        };
        // The default schedule is all-seven, so this is the raw calendar
        // fraction: 2.5 days into a 7-day week.
        assert_eq!(cells[0].period, Some(2.5 / 7.0));
        assert_eq!(cells[0].bar, Some(0.4), "the fill is unchanged");
    }

    /// Acceptance: the weekly window with a partial schedule produces the
    /// scheduled fraction, which differs from the raw calendar fraction; a
    /// non-weekly window with the same schedule uses the calendar fraction.
    #[test]
    fn the_schedule_reshapes_only_the_weekly_window() {
        let now = local_at(2025, 1, 8, 12);
        let mut cfg = cfg_with(&["a"]);
        cfg.providers.get_mut("a").unwrap().usage_schedule = monfri();
        let inst = WidgetInstanceConfig {
            accounts: vec![WidgetAccountSelection {
                provider_id: "a".into(),
                headlines: Some(vec!["window:weekly".into(), "window:five_hour".into()]),
            }],
            privacy: false,
        };
        let store = store_with(
            vec![snap(
                "a",
                vec![
                    bounded("weekly", local_at(2025, 1, 6, 0), local_at(2025, 1, 13, 0)),
                    // Both cells are evaluated at the projection's single
                    // `now`: 2 of the 5 hours elapsed by Wednesday noon.
                    UsageWindow {
                        label: "Five".into(),
                        ..bounded(
                            "five_hour",
                            local_at(2025, 1, 8, 10),
                            local_at(2025, 1, 8, 15),
                        )
                    },
                ],
            )],
            AggregateStatus::default(),
        );
        let p = project(Some(&inst), &store, &cfg, WidgetSize::Large, now);
        let RowState::Present { cells, .. } = &present(&p).rows[0].state else {
            panic!("present row");
        };
        let weekly = cells.iter().find(|c| c.label == "W").unwrap();
        let five = cells.iter().find(|c| c.label == "Five").unwrap();
        // Weekly paces Mon–Fri: half the working days by Wednesday noon —
        // different from the raw calendar's 2.5/7.
        assert_eq!(weekly.period, Some(0.5));
        assert_ne!(weekly.period, Some(2.5 / 7.0));
        // The five-hour window ignores the schedule entirely: 2 of 5 hours.
        assert_eq!(five.period, Some(0.4));
    }

    /// Acceptance: medium and small tiers carry no period fraction — the marker
    /// is gated to the large tier exactly like `bar` and `resets_at`.
    #[test]
    fn the_period_marker_is_gated_to_the_large_tier() {
        let store = store_with(
            vec![snap(
                "a",
                vec![bounded(
                    "weekly",
                    local_at(2025, 1, 6, 0),
                    local_at(2025, 1, 13, 0),
                )],
            )],
            AggregateStatus::default(),
        );
        let cfg = cfg_with(&["a"]);
        let inst = instance(&["a"]);
        for size in [WidgetSize::Medium, WidgetSize::Small] {
            let p = project(Some(&inst), &store, &cfg, size, local_at(2025, 1, 8, 12));
            let content = present(&p);
            let cell = match size {
                WidgetSize::Small => &content.worst.as_ref().expect("worst headline").cell,
                _ => {
                    let RowState::Present { cells, .. } = &content.rows[0].state else {
                        panic!("present row");
                    };
                    &cells[0]
                }
            };
            assert!(cell.period.is_none(), "{size:?} carries no marker");
        }
    }

    /// Acceptance: privacy redacts the period marker with the bar it rides on —
    /// with the bar hidden there is no track to mark, and reset text stays.
    #[test]
    fn privacy_redacts_the_period_marker() {
        let store = store_with(
            vec![snap(
                "a",
                vec![bounded(
                    "weekly",
                    local_at(2025, 1, 6, 0),
                    local_at(2025, 1, 13, 0),
                )],
            )],
            AggregateStatus::default(),
        );
        let cfg = cfg_with(&["a"]);
        let inst = WidgetInstanceConfig {
            privacy: true,
            ..instance(&["a"])
        };
        let p = project(
            Some(&inst),
            &store,
            &cfg,
            WidgetSize::Large,
            local_at(2025, 1, 8, 12),
        );
        let RowState::Present { cells, .. } = &present(&p).rows[0].state else {
            panic!("present row");
        };
        assert!(cells[0].bar.is_none(), "the bar is redacted");
        assert!(cells[0].period.is_none(), "the marker rides on the bar");
        assert!(cells[0].resets_at.is_some(), "the reset time survives");
    }

    /// A credit balance has no window, so no period regardless of tier.
    #[test]
    fn a_balance_cell_carries_no_period() {
        let credits = Credits {
            balance: 12.5,
            label: Some("Wallet".into()),
            unit: "USD".into(),
            used: None,
            granted: None,
            est_tokens_remaining: None,
        };
        let s = UsageSnapshot::ok("a", "a", vec![], Some(credits));
        let store = store_with(vec![s], AggregateStatus::default());
        let cfg = cfg_with(&["a"]);
        let inst = instance(&["a"]);
        let p = project(
            Some(&inst),
            &store,
            &cfg,
            WidgetSize::Large,
            local_at(2025, 1, 8, 12),
        );
        let RowState::Present { cells, .. } = &present(&p).rows[0].state else {
            panic!("present row");
        };
        assert_eq!(cells[0].label, "Wallet");
        assert!(cells[0].period.is_none());
    }

    /// The marker measures time, the fill measures quota: an overage reading
    /// clamps the bar to full but leaves the period fraction alone.
    #[test]
    fn the_period_marker_is_independent_of_an_overage_fill() {
        let mut w = bounded("weekly", local_at(2025, 1, 6, 0), local_at(2025, 1, 13, 0));
        w.used_pct = 150.0;
        let store = store_with(vec![snap("a", vec![w])], AggregateStatus::default());
        let cfg = cfg_with(&["a"]);
        let inst = instance(&["a"]);
        let p = project(
            Some(&inst),
            &store,
            &cfg,
            WidgetSize::Large,
            local_at(2025, 1, 8, 12),
        );
        let RowState::Present { cells, .. } = &present(&p).rows[0].state else {
            panic!("present row");
        };
        assert_eq!(cells[0].bar, Some(1.0), "the fill clamps to full");
        assert_eq!(cells[0].period, Some(2.5 / 7.0), "the marker does not");
    }

    /// The Rust port must agree with the desktop `periodProgress` bit-for-bit
    /// on shared fixtures: the same inputs go to both implementations and the
    /// fractions are compared. This is the drift guard for the one deliberate
    /// duplication the issue accepts — desktop stays on JS, the widget computes
    /// in quota-core.
    #[test]
    fn the_rust_fraction_matches_period_js_for_shared_fixtures() {
        let (start, end) = week();
        let monfri = monfri();
        let five_start = at(2025, 1, 6, 8);
        let fixtures: Vec<(UsageWindow, DateTime<Utc>, Option<UsageSchedule>)> = vec![
            // Calendar fraction, all clamps, missing bounds, degenerate spans.
            (bounded("weekly", start, end), at(2025, 1, 8, 12), None),
            (bounded("weekly", start, end), at(2025, 1, 6, 0), None),
            (bounded("weekly", start, end), at(2025, 1, 13, 0), None),
            (bounded("weekly", start, end), at(2025, 1, 20, 12), None),
            (bounded("weekly", start, end), at(2025, 1, 1, 12), None),
            // Scheduled weekly: working-day pacing, the weekend freeze, and a
            // mid-week period with partial boundary days.
            (
                bounded("weekly", start, end),
                at(2025, 1, 8, 12),
                Some(monfri),
            ),
            (
                bounded("weekly", start, end),
                at(2025, 1, 11, 12),
                Some(monfri),
            ),
            (
                bounded("weekly", at(2025, 1, 9, 12), at(2025, 1, 16, 12)),
                at(2025, 1, 9, 18),
                Some(monfri),
            ),
            (
                bounded("weekly", at(2025, 1, 9, 12), at(2025, 1, 16, 12)),
                at(2025, 1, 12, 12),
                Some(monfri),
            ),
            // Degenerate and absent bounds.
            (
                bounded("weekly", end, start),
                at(2025, 1, 8, 12),
                Some(monfri),
            ),
            {
                let mut w = bounded("weekly", start, end);
                w.period_start = None;
                (w, at(2025, 1, 8, 12), None)
            },
            // Non-weekly windows ignore the schedule.
            (
                bounded("five_hour", five_start, five_start + Duration::hours(5)),
                five_start + Duration::hours(2),
                Some(monfri),
            ),
            // All-seven schedule paces on the raw calendar.
            (
                bounded("weekly", start, end),
                at(2025, 1, 8, 12),
                Some(UsageSchedule::default()),
            ),
        ];

        let rust: Vec<Option<f64>> = fixtures
            .iter()
            .map(|(w, now, schedule)| period_progress(w, *now, schedule.as_ref(), &Utc))
            .collect();

        // The same fixtures, serialized as period.js reads them (ISO strings
        // and epoch-milliseconds), handed to node over stdin. TZ is pinned on
        // both sides so "local midnight" means the same day boundary to each.
        let payload: Vec<serde_json::Value> = fixtures
            .iter()
            .map(|(w, now, schedule)| {
                serde_json::json!({
                    "window": {
                        "metric_id": w.metric_id,
                        "period_start": w.period_start.map(|t| t.to_rfc3339()),
                        "resets_at": w.resets_at.map(|t| t.to_rfc3339()),
                    },
                    "now_ms": now.timestamp_millis(),
                    "schedule": schedule,
                })
            })
            .collect();

        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repo root resolves");
        let period_js = repo_root.join("src/lib/period.js");
        let harness = format!(
            "import {{ periodProgress }} from {}\n\
             let input = '';\n\
             process.stdin.setEncoding('utf8');\n\
             process.stdin.on('data', (d) => {{ input += d; }});\n\
             process.stdin.on('end', () => {{\n\
             \x20 const fixtures = JSON.parse(input);\n\
             \x20 const out = fixtures.map((f) => periodProgress(f.window, f.now_ms, f.schedule ?? null));\n\
             \x20 process.stdout.write(JSON.stringify(out));\n\
             }});\n",
            serde_json::to_string(&period_js).expect("path is a JSON string"),
        );
        let dir = tempfile::tempdir().unwrap();
        let harness_path = dir.path().join("parity.mjs");
        std::fs::write(&harness_path, harness).unwrap();

        let output = std::process::Command::new("node")
            .arg(&harness_path)
            .current_dir(&repo_root)
            .env("TZ", "UTC")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child
                    .stdin
                    .as_mut()
                    .expect("stdin piped")
                    .write_all(serde_json::to_string(&payload).unwrap().as_bytes())?;
                let out = child.wait_with_output()?;
                Ok(out)
            })
            .unwrap_or_else(|e| panic!("node is required for the period.js parity test: {e}"));

        assert!(
            output.status.success(),
            "node failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let js: Vec<Option<f64>> =
            serde_json::from_str(&String::from_utf8(output.stdout).expect("node prints JSON"))
                .expect("parity output is a JSON array of fractions");

        assert_eq!(rust.len(), js.len(), "both sides saw every fixture");
        for (i, (r, j)) in rust.iter().zip(js.iter()).enumerate() {
            match (r, j) {
                (None, None) => {}
                (Some(r), Some(j)) => assert!(
                    (r - j).abs() < 1e-9,
                    "fixture {i}: rust {r} != period.js {j}"
                ),
                (r, j) => panic!("fixture {i}: rust {r:?} != period.js {j:?}"),
            }
        }
    }

    /// The marker a large weekly cell exposes is the period fraction itself,
    /// bounded in 0.0..=1.0, so the host can draw it at `period × width`
    /// without re-deriving anything (ADR-0006).
    #[test]
    fn period_marker_is_exposed_on_a_large_weekly_cell() {
        let start = local_at(2025, 1, 6, 0);
        let end = local_at(2025, 1, 13, 0);
        let now = local_at(2025, 1, 8, 12);
        let store = store_with(
            vec![snap("a", vec![bounded("weekly", start, end)])],
            AggregateStatus::default(),
        );
        let cfg = cfg_with(&["a"]);
        let inst = instance(&["a"]);
        let p = project(Some(&inst), &store, &cfg, WidgetSize::Large, now);
        let RowState::Present { cells, .. } = &present(&p).rows[0].state else {
            panic!("present row");
        };
        let period = cells[0]
            .period
            .expect("a large weekly cell exposes a marker");
        assert!(
            (0.0..=1.0).contains(&period),
            "the fraction is bounded, got {period}"
        );
        // Default (all-seven) schedule: the raw calendar fraction — 2.5 of 7
        // days elapsed at Wednesday noon.
        assert!(
            (period - 2.5 / 7.0).abs() < 1e-9,
            "expected the calendar fraction 2.5/7, got {period}"
        );
    }
}
