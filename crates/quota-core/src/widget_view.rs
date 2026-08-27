//! The host-facing flattening of the [`widget`](crate::widget) projection.
//!
//! [`widget::project`] settles every widget *decision* — breakpoint selection,
//! per-instance selection, privacy redaction, removed accounts, the shared
//! aggregate colour — as rich Rust types. The native Android host (`docs/adr/
//! 0006-…`) only *draws*; it does not re-decide anything. This module is the
//! seam between the two: it loads the three persisted stores a cold widget
//! process can see, runs the projection, and flattens the result into a small,
//! explicitly-typed [`WidgetView`] whose JSON a Kotlin/Glance host parses with
//! nothing fancier than `org.json`.
//!
//! Keeping the flattening here — rather than re-deriving it in Kotlin — is what
//! stops the widget from drifting into "a new interpretation" (`widget.rs`):
//! the host receives finished labels, a finished status string, a finished bar
//! fraction and pre-formatted figures, and renders them verbatim. Everything
//! this module decides is exercised by the same Linux unit tests the rest of
//! `quota-core` is, so the native host inherits behaviour that CI already
//! guards instead of a parallel copy that can rot.
//!
//! ## What the host gets
//!
//! - [`render`] / [`render_json`] — the read path. Given the app config
//!   directory, an instance id and the launcher's current dimensions, it
//!   returns exactly what to draw, or one of the three honest placeholders.
//!   Never fetches, never touches a credential.
//! - [`config_options`] / [`save_instance`] / [`remove_instance`] — the
//!   configuration path the placement activity drives, seeding a fresh
//!   placement from the shared compact-summary selection
//!   ([`WidgetInstanceConfig::inherit_shared`]).

use crate::config::{Config, ConfigPresence};
use crate::model::Status;
use crate::snapshots::{SnapshotLoad, SnapshotStore};
use crate::widget::{
    self, HeadlineCell, HeadlineValue, RowState, WidgetConfigLoad, WidgetConfigStore,
    WidgetContent, WidgetInstanceConfig, WidgetProjection, WidgetSize, WidgetState, WorstHeadline,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The fully-flattened projection a native host renders. A single JSON object
/// with a `state` discriminator, so a host reads `state` and, when it is
/// `"content"`, reads `content`; the two placeholder states carry no content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetView {
    /// The resolved size tier, always present so the host sizes even a
    /// placeholder: `"small"`, `"medium"` or `"large"`.
    pub size: String,
    /// `"needs_configuration"`, `"no_data"` or `"content"`.
    pub state: String,
    /// Present only when `state == "content"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<WidgetContentView>,
}

/// The rendered content of a configured widget with data behind it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WidgetContentView {
    /// The shared aggregate status the widget colours itself from — `"ok"`,
    /// `"warn"`, `"critical"` or `"stale"`, taken verbatim from the read model.
    pub aggregate_status: String,
    /// The aggregate's loudest percentage, for a host that shows one.
    pub aggregate_pct: f64,
    /// How old the whole read model is, in whole seconds, for the "as of …"
    /// caption. `None` is not expected for content (the projection routes an
    /// unrefreshed store to `no_data`), but the host tolerates it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_age_secs: Option<i64>,
    /// Whether privacy mode redacted the figures below — the host may show a
    /// small "hidden" affordance.
    pub privacy: bool,
    /// The single worst selected headline — the small tier's whole body.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worst: Option<WorstView>,
    /// One row per selected account, in selection order. Empty on the small
    /// tier, which collapses to [`worst`](WidgetContentView::worst).
    pub rows: Vec<RowView>,
}

/// The worst selected headline, for the small tier and any headline glance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorstView {
    pub name: String,
    /// The account's status colour cue — retained even under privacy.
    pub status: String,
    pub cell: CellView,
}

/// One selected account's row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RowView {
    /// The account this row deep-links into (or asks to configure, when
    /// removed). Always present so a tap is never ambiguous.
    pub provider_id: String,
    /// The account's display name, retained under privacy.
    pub name: String,
    /// `true` when the selected account has disappeared from the read model —
    /// the host draws "Account removed—tap to configure", never a substitute.
    pub removed: bool,
    /// The row's own status colour cue, or `None` on a removed row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// The selected headline cells. Empty on a removed row.
    pub cells: Vec<CellView>,
}

/// One headline shown for an account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellView {
    /// The headline's label (a window name, or "Balance"). Kept under privacy —
    /// a name is not a figure.
    pub label: String,
    /// The pre-formatted figure the host draws verbatim (e.g. `"42%"` or
    /// `"12.5 USD"`), or `None` when privacy mode redacted it or the account
    /// reports none. Formatting lives here so it stays shared and tested rather
    /// than re-implemented per host.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The usage bar fraction in `0.0..=1.0` — `Some` only on the large tier
    /// for a non-redacted percentage headline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bar: Option<f64>,
    /// When this window resets, as epoch seconds — `Some` only on the large
    /// tier. A reset time is not a figure, so privacy keeps it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at_secs: Option<i64>,
}

impl WidgetView {
    /// A sized "Widget needs configuration" view — the placeholder a corrupt
    /// persisted input resolves to, distinct from the "no data" one an *absent*
    /// read model gives. The host still needs the size to draw the placeholder.
    fn needs_configuration(size: WidgetSize) -> Self {
        Self {
            size: size_str(size).to_string(),
            state: "needs_configuration".into(),
            content: None,
        }
    }

    /// Flatten a finished [`WidgetProjection`] for a host.
    pub fn from_projection(projection: &WidgetProjection) -> Self {
        let size = size_str(projection.size).to_string();
        match &projection.state {
            WidgetState::NeedsConfiguration => Self {
                size,
                state: "needs_configuration".into(),
                content: None,
            },
            WidgetState::NoData => Self {
                size,
                state: "no_data".into(),
                content: None,
            },
            WidgetState::Content(content) => Self {
                size,
                state: "content".into(),
                content: Some(content_view(content)),
            },
        }
    }
}

fn content_view(content: &WidgetContent) -> WidgetContentView {
    WidgetContentView {
        aggregate_status: status_str(content.aggregate.status).into(),
        aggregate_pct: content.aggregate.pct,
        data_age_secs: content.data_age.map(|d| d.num_seconds()),
        privacy: content.privacy,
        worst: content.worst.as_ref().map(worst_view),
        rows: content.rows.iter().map(row_view).collect(),
    }
}

fn worst_view(worst: &WorstHeadline) -> WorstView {
    WorstView {
        name: worst.name.clone(),
        status: status_str(worst.status).into(),
        cell: cell_view(&worst.cell),
    }
}

fn row_view(row: &widget::WidgetRow) -> RowView {
    match &row.state {
        RowState::Removed => RowView {
            provider_id: row.provider_id.clone(),
            name: row.name.clone(),
            removed: true,
            status: None,
            cells: Vec::new(),
        },
        RowState::Present { status, cells } => RowView {
            provider_id: row.provider_id.clone(),
            name: row.name.clone(),
            removed: false,
            status: Some(status_str(*status).into()),
            cells: cells.iter().map(cell_view).collect(),
        },
    }
}

fn cell_view(cell: &HeadlineCell) -> CellView {
    CellView {
        label: cell.label.clone(),
        value: cell.value.as_ref().map(format_value),
        bar: cell.bar,
        resets_at_secs: cell.resets_at.map(|t| t.timestamp()),
    }
}

/// Format a figure for display. The percentage is whole-number, matching the
/// compact surfaces; a balance shows its amount and unit ("12.5 USD"), trimming
/// a trailing `.0` so a round figure reads as an integer.
fn format_value(value: &HeadlineValue) -> String {
    match value {
        HeadlineValue::Usage { used_pct, .. } => format!("{}%", used_pct.round() as i64),
        HeadlineValue::Balance(credits) => {
            format!("{} {}", trim_amount(credits.balance), credits.unit)
        }
    }
}

/// A monetary amount without a needless trailing `.0`: `12.0 -> "12"`,
/// `12.5 -> "12.5"`, `12.34 -> "12.34"`.
fn trim_amount(amount: f64) -> String {
    let s = format!("{amount:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

fn size_str(size: WidgetSize) -> &'static str {
    match size {
        WidgetSize::Small => "small",
        WidgetSize::Medium => "medium",
        WidgetSize::Large => "large",
    }
}

fn status_str(status: Status) -> &'static str {
    match status {
        Status::Ok => "ok",
        Status::Warn => "warn",
        Status::Critical => "critical",
        Status::Stale => "stale",
    }
}

// ---- Read path -------------------------------------------------------------

/// Project one widget instance into a host-ready [`WidgetView`], loading the
/// three persisted stores from `dir`. Never fetches and never reads a
/// credential — this is the cold read a launcher performs with no app running.
///
/// `width_dp`/`height_dp` are the launcher's current cell dimensions (a
/// non-finite or degenerate value degrades to the small tier); `now` ages the
/// read model for the caption.
pub fn render(
    dir: &Path,
    instance_id: &str,
    width_dp: f64,
    height_dp: f64,
    now: DateTime<Utc>,
) -> WidgetView {
    let size = WidgetSize::from_dimensions(width_dp, height_dp);

    // Corrupt *or missing* persisted config is a configuration-level fault, not
    // an empty read model: the widget must surface "needs configuration" rather
    // than fall back to substituted defaults or pretend it is merely
    // un-refreshed. Each store is checked for that before projecting:
    //
    //  - a corrupt/unreadable *or absent* shared config would otherwise load the
    //    built-in default accounts (Claude/Codex, enabled in `Config::default`)
    //    and render readings for accounts the user never configured — the silent
    //    substitution issue #113 forbids for BOTH missing and corrupt config, so
    //    a `recovery`-only check is not enough (a missing file has no recovery).
    //    `Config::load_presence` is the primitive that tells the two failure
    //    shapes apart from a healthy persisted config;
    //  - a corrupt widget-preference file has lost every instance's selection;
    //  - a corrupt read model cannot be trusted to render.
    //
    // An *absent* read model is deliberately not corruption — it stays the
    // honest "No data—tap to refresh" the projection already produces (reached
    // only once a healthy config is present).
    let cfg = match Config::load_presence(dir) {
        ConfigPresence::Present(cfg) => cfg,
        ConfigPresence::Absent | ConfigPresence::Corrupt(_) => {
            return WidgetView::needs_configuration(size);
        }
    };

    let prefs = match WidgetConfigStore::load_state(dir) {
        WidgetConfigLoad::Corrupt => return WidgetView::needs_configuration(size),
        WidgetConfigLoad::Loaded(prefs) => prefs,
        WidgetConfigLoad::Absent => WidgetConfigStore::default(),
    };

    let snapshots = match SnapshotStore::load_state(dir) {
        SnapshotLoad::Corrupt => return WidgetView::needs_configuration(size),
        SnapshotLoad::Loaded(store) => store,
        // Absent → the empty read model → the projection routes to "no data".
        SnapshotLoad::Absent => SnapshotStore::default(),
    };

    let projection = widget::project(prefs.get(instance_id), &snapshots, &cfg, size, now);
    WidgetView::from_projection(&projection)
}

/// [`render`], serialized to a JSON string for the JNI/host boundary.
pub fn render_json(
    dir: &Path,
    instance_id: &str,
    width_dp: f64,
    height_dp: f64,
    now: DateTime<Utc>,
) -> String {
    let view = render(dir, instance_id, width_dp, height_dp, now);
    serde_json::to_string(&view).expect("widget view serializes")
}

// ---- Configuration path ----------------------------------------------------

/// One account the placement activity can offer, with whether it is currently
/// selected for this instance (an existing selection, or the shared default a
/// fresh placement inherits) and the display name to show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountOptionView {
    pub provider_id: String,
    pub name: String,
    /// Whether this account is in the instance's current (or inherited default)
    /// selection.
    pub selected: bool,
}

/// The options a placement activity renders: every enabled account with its
/// selected state, plus the instance's current privacy setting. A brand-new
/// placement (`instance_id` not yet saved) is seeded from the shared
/// compact-summary selection ([`WidgetInstanceConfig::inherit_shared`]) so the
/// user starts from a sensible default they can then diverge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigOptionsView {
    pub accounts: Vec<AccountOptionView>,
    pub privacy: bool,
}

/// Build the placement activity's options for `instance_id`, seeding an unsaved
/// instance from the shared compact-summary selection.
pub fn config_options(dir: &Path, instance_id: &str) -> ConfigOptionsView {
    let config_load = Config::load(dir);
    // A corrupt/unreadable shared config cannot be trusted to enumerate the real
    // accounts, and the built-in defaults must never stand in for them here — the
    // placement activity would otherwise offer accounts the user never created.
    // Offer nothing until the config is recovered.
    if config_load.recovery.is_some() {
        return ConfigOptionsView {
            accounts: Vec::new(),
            privacy: false,
        };
    }
    let cfg = config_load.config;
    let prefs = WidgetConfigStore::load(dir);
    // An existing instance keeps its own selection; a fresh placement inherits
    // the shared compact-summary selection as its initial choice.
    let current = prefs
        .get(instance_id)
        .cloned()
        .unwrap_or_else(|| WidgetInstanceConfig::inherit_shared(&cfg));
    let selected: std::collections::HashSet<&str> = current
        .accounts
        .iter()
        .map(|a| a.provider_id.as_str())
        .collect();

    let mut accounts = Vec::new();
    for (id, p) in cfg.providers.iter() {
        if !p.enabled {
            continue;
        }
        accounts.push(AccountOptionView {
            provider_id: id.clone(),
            name: p.label.clone().unwrap_or_else(|| id.clone()),
            selected: selected.contains(id.as_str()),
        });
    }
    ConfigOptionsView {
        accounts,
        privacy: current.privacy,
    }
}

/// [`config_options`], serialized for the JNI/host boundary.
pub fn config_options_json(dir: &Path, instance_id: &str) -> String {
    serde_json::to_string(&config_options(dir, instance_id)).expect("config options serialize")
}

/// Record (or replace) one instance's configuration and persist the store
/// atomically. Called when the placement activity's OK button is pressed.
pub fn save_instance(
    dir: &Path,
    instance_id: &str,
    config: WidgetInstanceConfig,
) -> std::io::Result<()> {
    let mut store = WidgetConfigStore::load(dir);
    store.set(instance_id, config);
    store.save(dir)
}

/// Parse a host-supplied instance config JSON and persist it. The JSON matches
/// [`WidgetInstanceConfig`] (`{ "accounts": [{ "provider_id": …,
/// "headlines": [...]? }], "privacy": bool }`). Returns the parse/IO error as a
/// string so the JNI shim can surface it without a panic crossing the boundary.
pub fn save_instance_json(dir: &Path, instance_id: &str, json: &str) -> Result<(), String> {
    let config: WidgetInstanceConfig =
        serde_json::from_str(json).map_err(|e| format!("invalid widget config: {e}"))?;
    save_instance(dir, instance_id, config).map_err(|e| e.to_string())
}

/// Forget an instance the launcher has removed, persisting the store.
pub fn remove_instance(dir: &Path, instance_id: &str) -> std::io::Result<()> {
    let mut store = WidgetConfigStore::load(dir);
    store.remove(instance_id);
    store.save(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use crate::model::{Credits, UsageSnapshot, UsageWindow};
    use crate::refresh::AggregateStatus;
    use chrono::Duration;

    fn window(metric_id: &str, label: &str, pct: f64) -> UsageWindow {
        UsageWindow {
            metric_id: metric_id.into(),
            label: label.into(),
            used_pct: pct,
            resets_at: Some(Utc::now() + Duration::hours(3)),
            ..Default::default()
        }
    }

    fn store_with(snapshots: Vec<UsageSnapshot>, aggregate: AggregateStatus) -> SnapshotStore {
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

    /// Write the three stores a widget reads into a temp dir, so [`render`] can
    /// be exercised through the same on-disk seam the host uses.
    fn seed_dir(
        cfg: &Config,
        snapshots: Vec<UsageSnapshot>,
        aggregate: AggregateStatus,
        instances: &[(&str, WidgetInstanceConfig)],
    ) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        cfg.save(dir.path()).unwrap();
        store_with(snapshots, aggregate).save(dir.path()).unwrap();
        let mut prefs = WidgetConfigStore::default();
        for (id, inst) in instances {
            prefs.set(*id, inst.clone());
        }
        prefs.save(dir.path()).unwrap();
        dir
    }

    fn instance(ids: &[&str]) -> WidgetInstanceConfig {
        WidgetInstanceConfig {
            accounts: ids
                .iter()
                .map(|id| widget::WidgetAccountSelection {
                    provider_id: (*id).into(),
                    headlines: None,
                })
                .collect(),
            privacy: false,
        }
    }

    #[test]
    fn a_large_content_view_carries_rows_bars_and_reset() {
        let cfg = cfg_with(&["a"]);
        let dir = seed_dir(
            &cfg,
            vec![UsageSnapshot::ok(
                "a",
                "Acme",
                vec![window("w", "Window", 42.0)],
                None,
            )],
            AggregateStatus {
                status: Status::Warn,
                pct: 42.0,
            },
            &[("id-1", instance(&["a"]))],
        );

        let view = render(dir.path(), "id-1", 300.0, 260.0, Utc::now());
        assert_eq!(view.size, "large");
        assert_eq!(view.state, "content");
        let content = view.content.unwrap();
        assert_eq!(content.aggregate_status, "warn");
        assert_eq!(content.rows.len(), 1);
        let row = &content.rows[0];
        assert!(!row.removed);
        assert_eq!(row.status.as_deref(), Some("ok"));
        let cell = &row.cells[0];
        assert_eq!(cell.label, "Window");
        assert_eq!(cell.value.as_deref(), Some("42%"));
        assert_eq!(cell.bar, Some(0.42));
        assert!(cell.resets_at_secs.is_some());
    }

    #[test]
    fn the_small_tier_flattens_to_a_worst_headline_and_no_rows() {
        let cfg = cfg_with(&["calm", "busy"]);
        let dir = seed_dir(
            &cfg,
            vec![
                UsageSnapshot::ok("calm", "Calm", vec![window("w", "Calm", 10.0)], None),
                UsageSnapshot::ok("busy", "Busy", vec![window("w", "Busy", 95.0)], None),
            ],
            AggregateStatus {
                status: Status::Critical,
                pct: 95.0,
            },
            &[("id-1", instance(&["calm", "busy"]))],
        );

        let view = render(dir.path(), "id-1", 120.0, 90.0, Utc::now());
        assert_eq!(view.size, "small");
        let content = view.content.unwrap();
        assert!(content.rows.is_empty(), "small tier lays out no rows");
        let worst = content.worst.expect("a worst headline");
        assert_eq!(worst.name, "Busy");
        assert_eq!(worst.status, "critical");
    }

    #[test]
    fn a_missing_instance_flattens_to_needs_configuration() {
        let cfg = cfg_with(&["a"]);
        let dir = seed_dir(
            &cfg,
            vec![UsageSnapshot::ok(
                "a",
                "A",
                vec![window("w", "W", 10.0)],
                None,
            )],
            AggregateStatus::default(),
            &[],
        );
        // The launcher knows of "ghost", but nothing was ever saved for it.
        let view = render(dir.path(), "ghost", 300.0, 260.0, Utc::now());
        assert_eq!(view.state, "needs_configuration");
        assert_eq!(view.size, "large");
        assert!(view.content.is_none());
    }

    #[test]
    fn a_configured_instance_with_no_read_model_flattens_to_no_data() {
        let cfg = cfg_with(&["a"]);
        let dir = tempfile::tempdir().unwrap();
        cfg.save(dir.path()).unwrap();
        // No snapshots.json written — the empty read model.
        let mut prefs = WidgetConfigStore::default();
        prefs.set("id-1", instance(&["a"]));
        prefs.save(dir.path()).unwrap();

        let view = render(dir.path(), "id-1", 200.0, 140.0, Utc::now());
        assert_eq!(view.state, "no_data");
        assert!(view.content.is_none());
    }

    #[test]
    fn a_removed_account_flattens_to_a_removed_row_with_no_cells() {
        let cfg = cfg_with(&["b"]); // "a" deleted from config too
        let dir = seed_dir(
            &cfg,
            vec![UsageSnapshot::ok(
                "b",
                "B",
                vec![window("w", "W", 50.0)],
                None,
            )],
            AggregateStatus::default(),
            &[("id-1", instance(&["a", "b"]))],
        );

        let content = render(dir.path(), "id-1", 200.0, 140.0, Utc::now())
            .content
            .unwrap();
        assert_eq!(content.rows.len(), 2);
        assert!(
            content.rows[0].removed,
            "the absent account is a removed row"
        );
        assert_eq!(content.rows[0].provider_id, "a");
        assert!(content.rows[0].status.is_none());
        assert!(content.rows[0].cells.is_empty());
        assert!(!content.rows[1].removed);
    }

    #[test]
    fn privacy_redacts_the_value_but_keeps_label_status_and_reset() {
        let cfg = cfg_with(&["a"]);
        let dir = seed_dir(
            &cfg,
            vec![UsageSnapshot::ok(
                "a",
                "A",
                vec![window("w", "Rolling", 42.0)],
                None,
            )],
            AggregateStatus::default(),
            &[(
                "id-1",
                WidgetInstanceConfig {
                    privacy: true,
                    ..instance(&["a"])
                },
            )],
        );

        let content = render(dir.path(), "id-1", 300.0, 260.0, Utc::now())
            .content
            .unwrap();
        assert!(content.privacy);
        let cell = &content.rows[0].cells[0];
        assert_eq!(cell.label, "Rolling", "the label survives redaction");
        assert!(cell.value.is_none(), "the figure is redacted");
        assert!(cell.bar.is_none(), "the bar (a figure) is redacted");
        assert!(cell.resets_at_secs.is_some(), "the reset time survives");
    }

    #[test]
    fn a_balance_value_formats_with_its_unit_and_trims_a_round_amount() {
        let round = Credits {
            balance: 12.0,
            label: Some("Wallet".into()),
            unit: "USD".into(),
            used: None,
            granted: None,
            est_tokens_remaining: None,
        };
        assert_eq!(format_value(&HeadlineValue::Balance(round)), "12 USD");
        let fractional = Credits {
            balance: 12.5,
            label: None,
            unit: "credits".into(),
            used: None,
            granted: None,
            est_tokens_remaining: None,
        };
        assert_eq!(
            format_value(&HeadlineValue::Balance(fractional)),
            "12.5 credits"
        );
    }

    #[test]
    fn the_view_json_round_trips_and_uses_stable_field_names() {
        let cfg = cfg_with(&["a"]);
        let dir = seed_dir(
            &cfg,
            vec![UsageSnapshot::ok(
                "a",
                "A",
                vec![window("w", "W", 42.0)],
                None,
            )],
            AggregateStatus::default(),
            &[("id-1", instance(&["a"]))],
        );
        let json = render_json(dir.path(), "id-1", 300.0, 260.0, Utc::now());
        // The host parses these keys with org.json; guard the wire names.
        assert!(json.contains("\"state\":\"content\""));
        assert!(json.contains("\"aggregate_status\""));
        assert!(json.contains("\"resets_at_secs\""));
        let back: WidgetView = serde_json::from_str(&json).unwrap();
        assert_eq!(back.state, "content");
    }

    // ---- Corruption vs. absence (the #113 blocking finding) ----------------

    use crate::snapshots::{SnapshotLoad, SnapshotStore as Snap};
    use crate::widget::{WidgetConfigLoad, WidgetConfigStore};

    /// Overwrite one of the three persisted stores with unparseable bytes.
    fn corrupt(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), "{ not valid json").unwrap();
    }

    /// A corrupt read model is a persisted-data fault, not an empty read model:
    /// the widget surfaces "needs configuration", never "no data" (which would
    /// invite a refresh over a file it could not trust) and never invented data.
    #[test]
    fn a_corrupt_read_model_flattens_to_needs_configuration_not_no_data() {
        let cfg = cfg_with(&["a"]);
        let dir = seed_dir(
            &cfg,
            vec![UsageSnapshot::ok(
                "a",
                "A",
                vec![window("w", "W", 10.0)],
                None,
            )],
            AggregateStatus::default(),
            &[("id-1", instance(&["a"]))],
        );
        // Sanity: healthy inputs render content.
        assert_eq!(
            render(dir.path(), "id-1", 300.0, 260.0, Utc::now()).state,
            "content"
        );

        corrupt(dir.path(), "snapshots.json");
        assert_eq!(SnapshotStore::load_state(dir.path()), SnapshotLoad::Corrupt);
        let view = render(dir.path(), "id-1", 300.0, 260.0, Utc::now());
        assert_eq!(view.state, "needs_configuration");
        assert_eq!(view.size, "large", "the placeholder is still sized");
        assert!(view.content.is_none());
    }

    /// An *absent* read model stays the honest "no data—tap to refresh"; this is
    /// the behaviour the corrupt-case must not regress.
    #[test]
    fn an_absent_read_model_still_flattens_to_no_data() {
        let cfg = cfg_with(&["a"]);
        let dir = tempfile::tempdir().unwrap();
        cfg.save(dir.path()).unwrap();
        let mut prefs = WidgetConfigStore::default();
        prefs.set("id-1", instance(&["a"]));
        prefs.save(dir.path()).unwrap();
        // No snapshots.json written at all — absent, not corrupt.
        assert_eq!(Snap::load_state(dir.path()), SnapshotLoad::Absent);

        let view = render(dir.path(), "id-1", 200.0, 140.0, Utc::now());
        assert_eq!(view.state, "no_data");
        assert!(view.content.is_none());
    }

    /// Corrupt widget preferences lose every instance's selection, so any
    /// instance the launcher asks about routes to needs-configuration.
    #[test]
    fn corrupt_widget_prefs_flatten_to_needs_configuration() {
        let cfg = cfg_with(&["a"]);
        let dir = seed_dir(
            &cfg,
            vec![UsageSnapshot::ok(
                "a",
                "A",
                vec![window("w", "W", 10.0)],
                None,
            )],
            AggregateStatus::default(),
            &[("id-1", instance(&["a"]))],
        );
        corrupt(dir.path(), "widgets.json");
        assert_eq!(
            WidgetConfigStore::load_state(dir.path()),
            WidgetConfigLoad::Corrupt
        );
        let view = render(dir.path(), "id-1", 300.0, 260.0, Utc::now());
        assert_eq!(view.state, "needs_configuration");
        assert!(view.content.is_none());
    }

    /// An *absent* shared config must not substitute the built-in defaults
    /// either. `Config::load` returns `recovery: None` for a missing file (a
    /// first run), so a recovery-only check would silently render readings for
    /// the pre-enabled Claude/Codex defaults — the regression the review flagged.
    /// The widget surfaces needs-configuration instead.
    #[test]
    fn an_absent_shared_config_flattens_to_needs_configuration_without_substituting_defaults() {
        // A snapshots file and a widget instance exist, but no shared config has
        // ever been written — the state a widget can cold-read before the app has
        // run. Write the two derived stores directly, leaving shared-config.json
        // absent.
        let dir = tempfile::tempdir().unwrap();
        store_with(
            vec![UsageSnapshot::ok(
                "claude",
                "Claude",
                vec![window("w", "W", 10.0)],
                None,
            )],
            AggregateStatus::default(),
        )
        .save(dir.path())
        .unwrap();
        let mut prefs = WidgetConfigStore::default();
        prefs.set("id-1", instance(&["claude"]));
        prefs.save(dir.path()).unwrap();
        assert!(
            !dir.path().join("shared-config.json").exists(),
            "the shared config is genuinely absent for this test"
        );

        let view = render(dir.path(), "id-1", 300.0, 260.0, Utc::now());
        assert_eq!(
            view.state, "needs_configuration",
            "an absent shared config must not substitute the Claude/Codex defaults"
        );
        assert!(view.content.is_none());
    }

    /// A corrupt shared config must not substitute the built-in default accounts:
    /// the widget surfaces needs-configuration instead of rendering readings for
    /// accounts (Claude/Codex, enabled in `Config::default`) the user never
    /// configured.
    #[test]
    fn a_corrupt_shared_config_flattens_to_needs_configuration_without_substituting_defaults() {
        let cfg = cfg_with(&["a"]);
        let dir = seed_dir(
            &cfg,
            vec![UsageSnapshot::ok(
                "a",
                "A",
                vec![window("w", "W", 10.0)],
                None,
            )],
            AggregateStatus::default(),
            &[("id-1", instance(&["a"]))],
        );
        corrupt(dir.path(), "shared-config.json");

        let view = render(dir.path(), "id-1", 300.0, 260.0, Utc::now());
        assert_eq!(view.state, "needs_configuration");
        assert!(view.content.is_none());

        // And the placement activity offers no accounts either, rather than the
        // built-in defaults it would get from `Config::default`.
        let opts = config_options(dir.path(), "id-1");
        assert!(
            opts.accounts.is_empty(),
            "no accounts are offered while the shared config is corrupt"
        );
    }

    // ---- Configuration path ------------------------------------------------

    #[test]
    fn config_options_for_a_fresh_placement_inherit_the_shared_selection() {
        let mut cfg = cfg_with(&["a", "b"]);
        cfg.providers.get_mut("a").unwrap().label = Some("Acme".into());
        // "b" opts out of the shared compact summary, so it is offered but not
        // pre-selected.
        cfg.providers.get_mut("b").unwrap().mini_summary_metrics = Some(vec![]);
        let dir = seed_dir(&cfg, vec![], AggregateStatus::default(), &[]);

        let opts = config_options(dir.path(), "brand-new");
        let a = opts.accounts.iter().find(|o| o.provider_id == "a").unwrap();
        let b = opts.accounts.iter().find(|o| o.provider_id == "b").unwrap();
        assert_eq!(a.name, "Acme", "label is offered as the display name");
        assert!(a.selected, "an in-summary account seeds selected");
        assert!(
            !b.selected,
            "an opted-out account is offered but not selected"
        );
        assert!(!opts.privacy);
    }

    #[test]
    fn config_options_for_a_saved_instance_reflect_its_own_selection() {
        let cfg = cfg_with(&["a", "b"]);
        let dir = seed_dir(
            &cfg,
            vec![],
            AggregateStatus::default(),
            &[(
                "id-1",
                WidgetInstanceConfig {
                    privacy: true,
                    ..instance(&["b"])
                },
            )],
        );
        let opts = config_options(dir.path(), "id-1");
        let a = opts.accounts.iter().find(|o| o.provider_id == "a").unwrap();
        let b = opts.accounts.iter().find(|o| o.provider_id == "b").unwrap();
        assert!(!a.selected);
        assert!(
            b.selected,
            "the saved selection wins over the shared default"
        );
        assert!(opts.privacy);
    }

    #[test]
    fn save_and_remove_instance_round_trip_through_the_store() {
        let cfg = cfg_with(&["a", "b"]);
        let dir = seed_dir(&cfg, vec![], AggregateStatus::default(), &[]);

        save_instance_json(
            dir.path(),
            "id-1",
            r#"{"accounts":[{"provider_id":"a"}],"privacy":true}"#,
        )
        .unwrap();
        let reloaded = WidgetConfigStore::load(dir.path());
        let saved = reloaded.get("id-1").unwrap();
        assert_eq!(saved.accounts.len(), 1);
        assert_eq!(saved.accounts[0].provider_id, "a");
        assert!(saved.privacy);

        remove_instance(dir.path(), "id-1").unwrap();
        assert!(WidgetConfigStore::load(dir.path()).get("id-1").is_none());
    }

    #[test]
    fn save_instance_json_rejects_malformed_input_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let err = save_instance_json(dir.path(), "id-1", "{ not json").unwrap_err();
        assert!(err.contains("invalid widget config"));
    }
}
