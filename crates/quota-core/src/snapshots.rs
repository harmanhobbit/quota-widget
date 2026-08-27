//! The persisted snapshot read model.
//!
//! Every surface that *displays* quota — the foreground Android app after a
//! cold start, and later every home-screen [`Widget instance`](../../CONTEXT.md)
//! — renders from this file, not from a live process. Nothing here fetches and
//! nothing here holds credentials: a refresh (`crate::refresh`) produces the
//! ordered snapshots and the one aggregate compact-surface status, and this
//! module is only how they survive the Tauri process exiting so a launcher can
//! render a widget with no app running at all.
//!
//! ## Corruption policy differs from configuration
//!
//! `shared-config.json` is *user-authored* and its keys name secrets that
//! cannot be enumerated back, so a file that cannot be parsed is kept and
//! blocks replacement (see `crate::shared_config`). Snapshots are the opposite:
//! wholly *derived* data that the next refresh regenerates, and per
//! `docs/adr/0006-…` "derived snapshot corruption is discarded". So [`load`]
//! never surfaces a recovery — a missing, unreadable, or malformed file simply
//! reads as an empty read model, and the next successful refresh overwrites it.
//! An empty read model is honest: it shows *no data yet*, never invented data.
//!
//! ## Absent is not corrupt, for a reader that must route on the difference
//!
//! Discarding corrupt data does *not* mean a reader cannot tell corruption from
//! absence. The home-screen widget must: an **absent** read model is the honest
//! "No data—tap to refresh", but a **corrupt** one is a persisted-data fault it
//! surfaces as "Widget needs configuration" rather than inviting a refresh over
//! a file it could not trust. [`load_state`] reports that three-way distinction
//! ([`SnapshotLoad`]); [`load`] stays the discard-to-empty convenience every
//! *refresh* path wants for its `prior` map, where absent and corrupt are alike
//! "start from nothing and let the next fetch repopulate".
//!
//! [`load`]: SnapshotStore::load
//! [`load_state`]: SnapshotStore::load_state

use crate::model::UsageSnapshot;
use crate::refresh::{AggregateStatus, RefreshOutcome};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// The file the read model is persisted to, alongside `shared-config.json` in
/// the app config directory. A widget process reads exactly this path.
const FILE_NAME: &str = "snapshots.json";

/// The last refresh's result, persisted for cold readers.
///
/// `snapshots` are already in configured display order (a refresh sorts before
/// it returns), so a reader renders the list top-to-bottom without re-sorting.
/// A snapshot whose latest fetch failed but which had an earlier success keeps
/// that earlier reading with the new error attached — a [`Stale
/// reading`](../../CONTEXT.md), visibly aged via its own `fetched_at`, never
/// presented as current. That merge is done by the refresh, not here; this
/// store only carries whatever the refresh produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapshotStore {
    /// Reserved for forward-compatible migrations. Fields are added under
    /// `#[serde(default)]`, so an older reader tolerates a newer file and a
    /// newer reader tolerates an older one; only a re-key or removal would need
    /// this bumped with a migration step.
    pub version: u32,
    /// When this read model was last written by a successful refresh. `None`
    /// before the first refresh ever completes — the empty read model. Distinct
    /// from each snapshot's own `fetched_at`, which ages a single stale reading.
    pub refreshed_at: Option<DateTime<Utc>>,
    /// The compact-surface status a widget colours itself from, folded over the
    /// snapshots below. Carried so a cold widget need not recompute the fold.
    pub aggregate: AggregateStatus,
    /// Every enabled provider's snapshot, in configured display order.
    pub snapshots: Vec<UsageSnapshot>,
}

impl Default for SnapshotStore {
    fn default() -> Self {
        Self {
            version: 1,
            refreshed_at: None,
            aggregate: AggregateStatus::default(),
            snapshots: Vec::new(),
        }
    }
}

/// The outcome of [`SnapshotStore::load_state`]: a readable read model, or which
/// of the two "nothing usable" shapes was on disk. A reader that renders state
/// from this — the home-screen widget — treats [`Absent`](SnapshotLoad::Absent)
/// as the honest "no data yet" and [`Corrupt`](SnapshotLoad::Corrupt) as a
/// persisted-data fault, so the two never collapse into one placeholder.
#[derive(Debug, Clone, PartialEq)]
pub enum SnapshotLoad {
    /// No read model has been written yet — a genuine "file not found". Routes to
    /// the widget's "No data—tap to refresh".
    Absent,
    /// A read model exists on disk but could not be read or parsed. The bytes are
    /// discarded (never recovered), and a widget routes this to "needs
    /// configuration" rather than pretending it is merely un-refreshed.
    Corrupt,
    /// A readable, parseable read model.
    Loaded(SnapshotStore),
}

impl SnapshotStore {
    /// Build a read model from a completed refresh, stamped with the current
    /// time. Use [`SnapshotStore::from_snapshots`] instead when the host has
    /// altered the snapshot list after the refresh (Android's post-refresh
    /// credential-failure replacements), so the stored aggregate matches the
    /// list actually persisted.
    pub fn from_outcome(outcome: &RefreshOutcome) -> Self {
        Self::at(Utc::now(), outcome.snapshots.clone(), outcome.aggregate)
    }

    /// Build a read model from an explicit snapshot list and aggregate, stamped
    /// now. The aggregate is the caller's responsibility — recompute it with
    /// [`aggregate_status`] whenever the list differs from the one the refresh
    /// returned.
    pub fn from_snapshots(snapshots: Vec<UsageSnapshot>, aggregate: AggregateStatus) -> Self {
        Self::at(Utc::now(), snapshots, aggregate)
    }

    fn at(now: DateTime<Utc>, snapshots: Vec<UsageSnapshot>, aggregate: AggregateStatus) -> Self {
        Self {
            version: 1,
            refreshed_at: Some(now),
            aggregate,
            snapshots,
        }
    }

    /// Read the persisted read model. A missing, unreadable, or malformed file
    /// all read as the empty read model — derived data is discarded on
    /// corruption, never recovered (see the module docs). A refresh building its
    /// `prior` map does not need to distinguish "no refresh yet" from "the file
    /// was corrupt": both mean *start from nothing and let the next fetch
    /// repopulate*. A reader that must route the two differently (the widget:
    /// absent → "no data", corrupt → "needs configuration") calls
    /// [`load_state`](SnapshotStore::load_state) instead.
    pub fn load(dir: &Path) -> Self {
        match Self::load_state(dir) {
            SnapshotLoad::Loaded(store) => store,
            // Both absent and corrupt discard to the empty read model here.
            SnapshotLoad::Absent | SnapshotLoad::Corrupt => Self::default(),
        }
    }

    /// Read the persisted read model, telling **absent** apart from **corrupt**.
    ///
    /// The bytes are still never *recovered* — a corrupt file is reported as
    /// [`SnapshotLoad::Corrupt`], not parsed leniently — but the caller learns
    /// which of the two failure shapes it hit so it can route on the difference.
    /// Only a genuine "no such file" is [`Absent`](SnapshotLoad::Absent); a file
    /// that exists but cannot be read (permissions, an I/O error) is
    /// [`Corrupt`](SnapshotLoad::Corrupt), the same as unparseable bytes, because
    /// both mean "a read model is on disk and we cannot trust what it says".
    pub fn load_state(dir: &Path) -> SnapshotLoad {
        match std::fs::read_to_string(dir.join(FILE_NAME)) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(store) => SnapshotLoad::Loaded(store),
                Err(_) => SnapshotLoad::Corrupt,
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => SnapshotLoad::Absent,
            Err(_) => SnapshotLoad::Corrupt,
        }
    }

    /// Write the read model atomically. Same temp-file-then-rename discipline as
    /// every other store in this crate, so a widget reading the file
    /// concurrently — or a process killed mid-write — never sees a torn or
    /// partial JSON document. Unlike configuration this never refuses to
    /// overwrite: the data is derived and the newest refresh always wins.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self).expect("snapshot store serializes");
        let path = dir.join(FILE_NAME);
        let tmp = dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }

    /// The stored snapshots keyed by provider id, as `crate::refresh::refresh`
    /// consumes them for `prior`. This is what carries a stale reading across a
    /// *cold start*: a fresh process loads the store, hands this map to the next
    /// refresh, and a provider that fails on that first post-launch fetch keeps
    /// the figures persisted before the process died rather than going blank.
    pub fn prior_map(&self) -> HashMap<String, UsageSnapshot> {
        self.snapshots
            .iter()
            .map(|s| (s.provider_id.clone(), s.clone()))
            .collect()
    }

    /// How old the whole read model is relative to `now`, or `None` before any
    /// refresh has completed. A reader uses this to caption the read model as a
    /// whole ("as of 20 min ago"); a single stale card's age comes from its own
    /// `fetched_at`, which can be older still.
    pub fn age(&self, now: DateTime<Utc>) -> Option<chrono::Duration> {
        self.refreshed_at.map(|at| now - at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ProviderConfig};
    use crate::model::{FetchError, Status, UsageWindow};
    use crate::refresh::aggregate_status;

    fn window(pct: f64) -> UsageWindow {
        UsageWindow {
            metric_id: "w".into(),
            label: "w".into(),
            used_pct: pct,
            ..Default::default()
        }
    }

    fn ok(id: &str, pct: f64) -> UsageSnapshot {
        UsageSnapshot::ok(id, id, vec![window(pct)], None)
    }

    fn cfg() -> Config {
        let mut cfg = Config::default();
        for key in ["openrouter", "claude"] {
            cfg.providers.insert(
                key.into(),
                ProviderConfig {
                    enabled: true,
                    ..Default::default()
                },
            );
        }
        cfg
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::from_snapshots(
            vec![ok("openrouter", 42.0)],
            AggregateStatus {
                status: Status::Warn,
                pct: 42.0,
            },
        );
        store.save(dir.path()).unwrap();
        assert_eq!(SnapshotStore::load(dir.path()), store);
    }

    #[test]
    fn a_missing_file_is_the_empty_read_model() {
        let dir = tempfile::tempdir().unwrap();
        let store = SnapshotStore::load(dir.path());
        assert!(store.snapshots.is_empty());
        assert!(store.refreshed_at.is_none());
        assert_eq!(store, SnapshotStore::default());
    }

    /// Derived data: a file we cannot parse is discarded to the empty read
    /// model, not kept-and-blocking the way a corrupt config is.
    #[test]
    fn a_corrupt_file_is_discarded_not_recovered() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), "{ this is not json").unwrap();
        let store = SnapshotStore::load(dir.path());
        assert_eq!(store, SnapshotStore::default());
        // And saving over it just works — no overwrite refusal for derived data.
        SnapshotStore::from_snapshots(vec![ok("openrouter", 1.0)], AggregateStatus::default())
            .save(dir.path())
            .unwrap();
        assert_eq!(SnapshotStore::load(dir.path()).snapshots.len(), 1);
    }

    /// `load_state` tells absent, corrupt and loaded apart — the distinction the
    /// widget routes on ("no data" vs "needs configuration"), even though `load`
    /// still collapses the two failures to the empty read model.
    #[test]
    fn load_state_distinguishes_absent_corrupt_and_loaded() {
        let dir = tempfile::tempdir().unwrap();
        // No file yet: absent, not corrupt.
        assert_eq!(SnapshotStore::load_state(dir.path()), SnapshotLoad::Absent);

        // A malformed file is corrupt, not absent — and `load` still discards it.
        std::fs::write(dir.path().join(FILE_NAME), "{ not json").unwrap();
        assert_eq!(SnapshotStore::load_state(dir.path()), SnapshotLoad::Corrupt);
        assert_eq!(SnapshotStore::load(dir.path()), SnapshotStore::default());

        // A real read model loads.
        let written =
            SnapshotStore::from_snapshots(vec![ok("openrouter", 7.0)], AggregateStatus::default());
        written.save(dir.path()).unwrap();
        assert_eq!(
            SnapshotStore::load_state(dir.path()),
            SnapshotLoad::Loaded(written)
        );
    }

    /// Acceptance #1: a cold process renders the same last-known state a live
    /// process wrote, without any live process in between. Persisting is the
    /// whole mechanism, so the assertion is simply load-after-save equality on
    /// the visible fields.
    #[test]
    fn a_cold_reader_sees_the_last_written_state() {
        let dir = tempfile::tempdir().unwrap();
        let written = SnapshotStore::from_snapshots(
            vec![ok("openrouter", 30.0), ok("claude", 70.0)],
            AggregateStatus {
                status: Status::Warn,
                pct: 70.0,
            },
        );
        written.save(dir.path()).unwrap();

        // Nothing runs in between — a brand-new load off disk.
        let cold = SnapshotStore::load(dir.path());
        assert_eq!(cold.snapshots, written.snapshots);
        assert_eq!(cold.aggregate, written.aggregate);
        assert!(cold.refreshed_at.is_some());
    }

    #[test]
    fn prior_map_keys_snapshots_by_provider_for_the_next_refresh() {
        let store = SnapshotStore::from_snapshots(
            vec![ok("openrouter", 10.0), ok("claude", 20.0)],
            AggregateStatus::default(),
        );
        let prior = store.prior_map();
        assert_eq!(prior.len(), 2);
        assert_eq!(prior["claude"].windows[0].used_pct, 20.0);
    }

    #[test]
    fn from_outcome_carries_the_refresh_aggregate_and_order() {
        let outcome = RefreshOutcome {
            snapshots: vec![ok("claude", 90.0), ok("openrouter", 5.0)],
            aggregate: AggregateStatus {
                status: Status::Critical,
                pct: 90.0,
            },
            ..Default::default()
        };
        let store = SnapshotStore::from_outcome(&outcome);
        assert_eq!(store.aggregate.status, Status::Critical);
        assert_eq!(
            store
                .snapshots
                .iter()
                .map(|s| s.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["claude", "openrouter"],
        );
    }

    /// The aggregate a host recomputes with `aggregate_status` over an
    /// altered list is what gets stored — a fetch failure introduced after the
    /// refresh raises the persisted colour to Stale rather than leaving the
    /// pre-replacement colour behind.
    #[test]
    fn recomputed_aggregate_reflects_a_post_refresh_failure() {
        let cfg = cfg();
        let calm = vec![ok("openrouter", 10.0)];
        assert_eq!(aggregate_status(&calm, &cfg).status, Status::Ok);

        let failed = vec![UsageSnapshot::failed(
            "openrouter",
            "openrouter",
            FetchError::Unavailable("keystore".into()),
        )];
        let store = SnapshotStore::from_snapshots(failed.clone(), aggregate_status(&failed, &cfg));
        assert_eq!(store.aggregate.status, Status::Stale);
    }
}
