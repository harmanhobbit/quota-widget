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

use crate::config::Config;
use crate::model::UsageSnapshot;
use crate::refresh::{aggregate_status, AggregateStatus, RefreshOutcome};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// The file the read model is persisted to, alongside `shared-config.json` in
/// the app config directory. A widget process reads exactly this path.
const FILE_NAME: &str = "snapshots.json";

/// The file holding the last allocated attempt generation — a lock-serialized,
/// persisted, strictly increasing counter (see [`next_generation`]). A separate
/// file, not a field of the store, because it must advance when a pass
/// *allocates* — before its fetches — while the store is written only when the
/// pass finishes.
const GENERATION_NAME: &str = "snapshots.json.gen";

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
    /// Per provider, the attempt generation of the pass that produced the
    /// stored snapshot (see [`merge_and_store`]). A strict counter allocated
    /// under the store lock before the pass's first fetch — never the wall
    /// clock, whose collisions and adjustments are not causal order. A store
    /// written before this field existed, or by a build that stamped
    /// generations with wall-clock times, carries no usable entries; those
    /// read as the oldest possible generation, so the next allocated one
    /// supersedes them.
    #[serde(default, deserialize_with = "deserialize_generations")]
    pub generations: HashMap<String, u64>,
    /// The attempt generation of the pass that last decided **which providers
    /// the model contains**. Membership changes — a provider added by
    /// appearing in a pass's outcome, or removed by dropping out of it — are
    /// authoritative only from a pass newer than this; an older pass's
    /// outcome was composed from an older configuration, and letting it
    /// decide membership would let it erase a provider a newer pass stored,
    /// or re-add one a newer pass removed (see [`merge_read_model`]).
    #[serde(default)]
    pub membership_generation: u64,
}

impl Default for SnapshotStore {
    fn default() -> Self {
        Self {
            version: 1,
            refreshed_at: None,
            aggregate: AggregateStatus::default(),
            snapshots: Vec::new(),
            generations: HashMap::new(),
            membership_generation: 0,
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
            generations: HashMap::new(),
            membership_generation: 0,
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
    /// overwrite: the data is derived, and every writer goes through
    /// [`merge_and_store`], which keeps the fresher observation per provider
    /// (see [`fresher`]) — a late write composed from stale prior state can
    /// add its error but never erases newer figures.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self).expect("snapshot store serializes");
        let path = dir.join(FILE_NAME);
        let tmp = dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }

    /// Merge a freshly composed snapshot list into the **current** persisted
    /// read model and store the result, returning the store as written.
    ///
    /// Every writer of the read model — the foreground app's `refresh_once`,
    /// the WorkManager worker behind the manual and periodic refreshes, and a
    /// cold widget process — composes from its own view of `prior`, and those
    /// views can be arbitrarily stale relative to each other: the foreground's
    /// fetch may have started before a worker's finished. A whole-file
    /// overwrite therefore lets a *late writer composed from stale prior state*
    /// regress the model. This closes that per provider: [`fresher`] decides
    /// between the incoming snapshot and the one currently on disk by attempt
    /// generation, and [`merge_read_model`] decides membership by it too, so a
    /// causally older write changes nothing.
    ///
    /// `attempt` is the incoming pass's generation — the strict counter
    /// [`next_generation`] allocated for it *before its first fetch*.
    /// Wall-clock time is not a causal order (concurrent passes can start
    /// within the same instant, and clock adjustments go backwards); the
    /// counter is total across processes and restarts.
    ///
    /// The read-merge-write runs under an exclusive [`LOCK_NAME`] lock, so
    /// concurrent entries in one process — and the foreground and a widget-only
    /// worker process — serialize here instead of racing the file itself. The
    /// lock is deliberately held only for this merge-and-write (never across a
    /// fetch): it is released on drop, and the kernel releases it outright if a
    /// writer dies holding it, so a crash cannot leave the store stuck.
    pub fn merge_and_store(
        dir: &Path,
        incoming: Vec<UsageSnapshot>,
        attempt: u64,
        cfg: &Config,
    ) -> std::io::Result<Self> {
        let _lock = acquire_store_lock(dir)?;
        let store = Self::derive_merged(dir, incoming, attempt, cfg);
        store.save(dir)?;
        Ok(store)
    }

    /// The causally merged read model for `incoming`, computed against the
    /// store as it currently reads — **without writing**. The locked
    /// [`merge_and_store`] is this plus the save; a failed store write falls
    /// back to this so the in-memory state and the open webview derive the
    /// same merged truth instead of ever publishing an unmerged older result.
    /// The merged list is re-sorted into configured display order and the
    /// aggregate recomputed over it — the stored order and colour must match
    /// the stored cards.
    pub fn derive_merged(
        dir: &Path,
        incoming: Vec<UsageSnapshot>,
        attempt: u64,
        cfg: &Config,
    ) -> Self {
        let current = Self::load(dir);
        let merged = merge_read_model(
            &current.snapshots,
            &current.generations,
            current.membership_generation,
            incoming,
            attempt,
        );
        let mut snapshots = merged.snapshots;
        cfg.sort_snapshots(&mut snapshots);
        let aggregate = aggregate_status(&snapshots, cfg);
        let mut store = Self::from_snapshots(snapshots, aggregate);
        store.generations = merged.generations;
        store.membership_generation = merged.membership_generation;
        store
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

/// The advisory lock guarding the read-merge-write in
/// [`SnapshotStore::merge_and_store`]. A separate file — not `snapshots.json`
/// itself — because `save` replaces that file by rename, which would silently
/// drop a lock held on the old inode. Kernel-owned: it disappears the moment
/// the last holder (or a crashed writer's process) closes it, so it can never
/// go stale.
const LOCK_NAME: &str = "snapshots.json.lock";

/// Take the exclusive store lock, blocking until any other writer — the
/// foreground app, a WorkManager worker thread, or a cold widget process —
/// has finished its own read-merge-write.
fn acquire_store_lock(dir: &Path) -> std::io::Result<std::fs::File> {
    use fs2::FileExt;
    std::fs::create_dir_all(dir)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LOCK_NAME))?;
    file.lock_exclusive()?;
    Ok(file)
}

/// Allocate the next attempt generation: a lock-serialized, persisted,
/// strictly increasing counter — never the wall clock. `Utc::now()` is not a
/// causal order: passes can start within the same instant, and a clock
/// adjustment moves time backwards, either of which would let an older pass's
/// results masquerade as newer. Allocation happens under the store lock,
/// before a pass fetches anything, and the counter persists across restarts,
/// so generations are total across processes and reboots.
pub fn next_generation(dir: &Path) -> std::io::Result<u64> {
    let _lock = acquire_store_lock(dir)?;
    let last = std::fs::read_to_string(dir.join(GENERATION_NAME))
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let next = last.saturating_add(1);
    // Temp-then-rename: a torn counter write must not rewind generations.
    let tmp = dir.join(format!("{GENERATION_NAME}.tmp"));
    std::fs::write(&tmp, next.to_string())?;
    std::fs::rename(tmp, dir.join(GENERATION_NAME))?;
    Ok(next)
}

/// Generations were once stamped with wall-clock `DateTime<Utc>` values; such
/// entries are not counters. They deserialize as absent — the oldest possible
/// generation — so the next allocated generation supersedes them and the
/// store file is not discarded whole.
fn deserialize_generations<'de, D>(deserializer: D) -> Result<HashMap<String, u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: HashMap<String, serde_json::Value> = serde::Deserialize::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter_map(|(id, value)| value.as_u64().map(|generation| (id, generation)))
        .collect())
}

/// The result of [`merge_read_model`]: the merged snapshots, the generation
/// each kept one now carries, and the membership generation the model
/// continues from.
pub struct MergedReadModel {
    pub snapshots: Vec<UsageSnapshot>,
    pub generations: HashMap<String, u64>,
    pub membership_generation: u64,
}

/// The fresher of two observations of the same provider, deciding which one a
/// merge keeps. The **attempt generation** — when each pass began, allocated
/// from the persisted monotonic counter before its first fetch — is the sole
/// ordering key, for figures and failure reason alike. Completion times
/// (`fetched_at`) cannot order concurrent writers: a pass that began earlier
/// can complete later, so a later-completing snapshot may belong to an
/// *older* generation — ordering by it would let that older pass's figures
/// or failure displace a newer pass's success. `fetched_at` is display
/// metadata of whichever observation won (the figures' own success, ageing
/// the card); it never decides the race.
///
/// - The candidate began later → the candidate wins wholesale: its figures and
///   its error. A figure-less failure means the newest attempt observed a
///   provider it never successfully read — the honest state is that failure,
///   and the next successful pass repopulates the figures.
/// - The candidate began earlier → the current wins wholesale: an in-flight
///   older-generation write — failing *or succeeding* late — modifies
///   nothing.
/// - Equal generations cannot be produced by [`next_generation`]; should one
///   appear anyway, the stored snapshot stands — an incoming write never
///   wins a tie it did not causally earn.
fn fresher(
    current: &UsageSnapshot,
    current_attempt: u64,
    candidate: &UsageSnapshot,
    candidate_attempt: u64,
) -> UsageSnapshot {
    if candidate_attempt > current_attempt {
        candidate.clone()
    } else {
        current.clone()
    }
}

/// Merge a freshly composed snapshot list into the current read model, per
/// provider, by [`fresher`] — and decide membership causally. A provider the
/// incoming pass contains but the model does not is an *addition*; a provider
/// the model contains but the pass does not is a *removal*. Both are
/// membership decisions, and they are authoritative only from a pass whose
/// generation is newer than [`SnapshotStore::membership_generation`]: an older
/// pass's outcome was composed from an older configuration, and letting it
/// decide membership would erase a provider a newer pass stored, or re-add
/// one a newer pass removed. Per-provider freshness still applies within that
/// rule — an older pass's fresher figures for a provider that remains in the
/// model do land.
///
/// Incoming order is preserved for the providers it contains (callers re-sort
/// afterwards); when the pass is not authoritative, providers it could not
/// see are appended as stored.
pub fn merge_read_model(
    current: &[UsageSnapshot],
    current_generations: &HashMap<String, u64>,
    membership_generation: u64,
    incoming: Vec<UsageSnapshot>,
    attempt: u64,
) -> MergedReadModel {
    let authoritative = attempt > membership_generation;
    let mut generations = current_generations.clone();
    let mut merged = Vec::with_capacity(incoming.len());
    for candidate in incoming {
        let id = candidate.provider_id.clone();
        match current.iter().find(|s| s.provider_id == id) {
            Some(current_snapshot) => {
                let current_attempt = current_generations.get(&id).copied().unwrap_or(0);
                let kept = fresher(current_snapshot, current_attempt, &candidate, attempt);
                generations.insert(id, current_attempt.max(attempt));
                merged.push(kept);
            }
            // Not in the model: adding it is a membership decision, reserved
            // for a pass newer than the last one that decided membership.
            None if authoritative => {
                generations.insert(id.clone(), attempt);
                merged.push(candidate);
            }
            // An older pass cannot re-add what a newer pass removed.
            None => {}
        }
    }
    let membership_generation = if authoritative {
        attempt
    } else {
        membership_generation
    };
    if authoritative {
        // The newest membership decision: providers the pass no longer
        // contains drop out, with their generations.
        generations.retain(|id, _| merged.iter().any(|s| &s.provider_id == id));
    } else {
        // The current membership stands: providers the older pass could not
        // see stay exactly as stored.
        for current_snapshot in current {
            if !merged
                .iter()
                .any(|s| s.provider_id == current_snapshot.provider_id)
            {
                merged.push(current_snapshot.clone());
            }
        }
    }
    MergedReadModel {
        snapshots: merged,
        generations,
        membership_generation,
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

    // ---- the writer race: merge_read_model / merge_and_store ----------------
    //
    // The foreground app, the periodic WorkManager worker, and the manual
    // durable worker each compose from their own view of `prior` and write the
    // same persisted read model. These tests pin the rules that make those
    // concurrent writes safe: causal order is the ATTEMPT GENERATION — a
    // strict counter allocated under the store lock before each pass's first
    // fetch, never the wall clock — figures and failure reason alike, and
    // membership (which providers the model contains) is decided only by a
    // causally newer pass. Every timestamp below is explicit, so all cases
    // are deterministic.

    fn with_fetched_at(mut s: UsageSnapshot, at: DateTime<Utc>) -> UsageSnapshot {
        s.fetched_at = at;
        s
    }

    /// A store's per-provider generation map: the counter value each
    /// producing pass was allocated.
    fn gen_map(entries: &[(&str, u64)]) -> HashMap<String, u64> {
        entries
            .iter()
            .map(|(id, generation)| (id.to_string(), *generation))
            .collect()
    }

    /// What compose produces when a later fetch fails after a success: the
    /// success's figures and their timestamp, the newer error attached.
    fn stale_at(id: &str, pct: f64, figures_at: DateTime<Utc>, msg: &str) -> UsageSnapshot {
        let mut s = ok(id, pct);
        s.fetched_at = figures_at;
        s.error = Some(FetchError::Network(msg.into()));
        s
    }

    /// A failed first fetch: no figures were invented, and the attempt's own
    /// completion time is stamped.
    fn failed_at(id: &str, at: DateTime<Utc>, msg: &str) -> UsageSnapshot {
        let mut s = UsageSnapshot::failed(id, id, FetchError::Network(msg.into()));
        s.fetched_at = at;
        s
    }

    /// The allocator is a counter, not a clock: consecutive allocations are
    /// exactly one apart, whatever the wall clock does between them — a clock
    /// step-back cannot rewind generations and simultaneous starts cannot
    /// collide — and the counter persists, so generations are total across
    /// processes and restarts.
    #[test]
    fn generations_are_persisted_and_strictly_monotonic() {
        let dir = tempfile::tempdir().unwrap();
        let g1 = next_generation(dir.path()).unwrap();
        let g2 = next_generation(dir.path()).unwrap();
        let g3 = next_generation(dir.path()).unwrap();
        assert_eq!((g1, g2, g3), (1, 2, 3));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("snapshots.json.gen")).unwrap(),
            "3",
            "the counter is persisted, so a restart cannot rewind it",
        );
    }

    /// Equal generations cannot be produced by the allocator; should one
    /// appear anyway (a store carried over, a defensive default), the STORED
    /// snapshot stands — an incoming write never wins a tie it did not
    /// causally earn, whichever side carries figures.
    #[test]
    fn equal_generations_keep_the_stored_snapshot() {
        let now = Utc::now();
        let current_success = vec![with_fetched_at(ok("codex", 25.0), now)];
        let incoming_failure = vec![failed_at("codex", now, "same-generation failure")];
        let merged = merge_read_model(
            &current_success,
            &gen_map(&[("codex", 5)]),
            5,
            incoming_failure,
            5,
        );
        assert_eq!(merged.snapshots[0].windows[0].used_pct, 25.0);
        assert!(merged.snapshots[0].error.is_none());

        let current_failure = vec![failed_at("codex", now, "stored failure")];
        let incoming_success = vec![with_fetched_at(ok("codex", 20.0), now)];
        let merged = merge_read_model(
            &current_failure,
            &gen_map(&[("codex", 5)]),
            5,
            incoming_success,
            5,
        );
        assert_eq!(
            merged.snapshots[0].error,
            Some(FetchError::Network("stored failure".into())),
        );
        assert!(merged.snapshots[0].windows.is_empty());
    }

    /// THE regression test for the r5 blocker. Request A begins first (no
    /// prior figures for the provider), request B begins later and persists a
    /// clean success, and A — in flight the whole time — fails last. A's
    /// failure carries a NEWER completion timestamp than B's figures, so only
    /// the attempt generation can keep B's cards clean: no stale error, no
    /// stale aggregate, in either persistence order.
    #[test]
    fn an_older_in_flight_failure_cannot_modify_a_later_success() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg();
        let a_attempt = next_generation(dir.path()).unwrap();
        let b_attempt = next_generation(dir.path()).unwrap();
        // B's figures were fetched after B began; A's failure completed after
        // B's figures were even fetched — completion order is exactly wrong.
        let b_success = with_fetched_at(ok("codex", 25.0), Utc::now());
        let a_failure = failed_at("codex", Utc::now(), "A failed");

        // Order 1: B's success lands first; A's late failure merges second.
        SnapshotStore::merge_and_store(dir.path(), vec![b_success.clone()], b_attempt, &cfg)
            .unwrap();
        let store =
            SnapshotStore::merge_and_store(dir.path(), vec![a_failure.clone()], a_attempt, &cfg)
                .unwrap();
        assert_eq!(
            store.snapshots[0].windows[0].used_pct, 25.0,
            "B's clean figures unchanged",
        );
        assert!(
            store.snapshots[0].error.is_none(),
            "A's older in-flight failure adds no stale error",
        );
        assert_eq!(
            store.aggregate.status,
            Status::Ok,
            "the model is not marked stale",
        );
        assert_eq!(store.generations["codex"], b_attempt);

        // Order 2: A's failure lands first; B's success merges after it.
        let dir = tempfile::tempdir().unwrap();
        SnapshotStore::merge_and_store(dir.path(), vec![a_failure], a_attempt, &cfg).unwrap();
        let store =
            SnapshotStore::merge_and_store(dir.path(), vec![b_success], b_attempt, &cfg).unwrap();
        assert_eq!(store.snapshots[0].windows[0].used_pct, 25.0);
        assert!(store.snapshots[0].error.is_none());
        assert_eq!(store.aggregate.status, Status::Ok);
        assert_eq!(store.generations["codex"], b_attempt);
    }

    /// THE regression test for the r6 blocker. A begins first, B begins later
    /// and persists a clean success, then A SUCCEEDS with a completion
    /// timestamp newer than B's figures' fetched_at. Under completion-time
    /// ordering A's figures would win; under generation ordering B's clean
    /// figures, error state, aggregate and generation are unchanged, in both
    /// persistence orders.
    #[test]
    fn an_older_generation_success_cannot_regress_a_newer_success() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg();
        let a_attempt = next_generation(dir.path()).unwrap();
        let b_attempt = next_generation(dir.path()).unwrap();
        let b_success = with_fetched_at(ok("codex", 25.0), Utc::now());
        let a_success = with_fetched_at(ok("codex", 20.0), Utc::now());

        // Order 1: B lands first; A's late (older-generation) success merges second.
        SnapshotStore::merge_and_store(dir.path(), vec![b_success.clone()], b_attempt, &cfg)
            .unwrap();
        let store =
            SnapshotStore::merge_and_store(dir.path(), vec![a_success.clone()], a_attempt, &cfg)
                .unwrap();
        assert_eq!(
            store.snapshots[0].windows[0].used_pct, 25.0,
            "B's figures are not regressed by A's later-completing success",
        );
        assert!(
            store.snapshots[0].error.is_none(),
            "B's clean error state holds"
        );
        assert_eq!(store.aggregate.status, Status::Ok);
        assert_eq!(store.generations["codex"], b_attempt);

        // Order 2: A's late success lands first; B's younger success merges after it.
        let dir = tempfile::tempdir().unwrap();
        SnapshotStore::merge_and_store(dir.path(), vec![a_success], a_attempt, &cfg).unwrap();
        let store =
            SnapshotStore::merge_and_store(dir.path(), vec![b_success], b_attempt, &cfg).unwrap();
        assert_eq!(store.snapshots[0].windows[0].used_pct, 25.0);
        assert!(store.snapshots[0].error.is_none());
        assert_eq!(store.aggregate.status, Status::Ok);
        assert_eq!(store.generations["codex"], b_attempt);
    }

    /// A stale write composed from prior state older than what another writer
    /// already persisted cannot regress it — its figures are causally older
    /// (its pass began earlier), so its error is discarded with them.
    #[test]
    fn a_late_stale_write_cannot_regress_a_newer_success() {
        let t0 = Utc::now() - chrono::Duration::seconds(120);
        let t4 = Utc::now() - chrono::Duration::seconds(30);
        // The current store: a success fetched at T4 by a pass with generation 3.
        let current = vec![with_fetched_at(ok("codex", 25.0), t4)];
        let generations = gen_map(&[("codex", 3)]);
        // The incoming pass has generation 1, composed from a T0 success, and
        // failed — stale figures from T0 with its error attached.
        let incoming = vec![stale_at("codex", 20.0, t0, "late failure")];

        let merged = merge_read_model(&current, &generations, 3, incoming, 1);

        assert_eq!(
            merged.snapshots[0].windows[0].used_pct, 25.0,
            "the newer success's figures survive the late stale write",
        );
        assert!(
            merged.snapshots[0].error.is_none(),
            "the causally older write adds no error to a newer success",
        );
        assert_eq!(merged.snapshots[0].fetched_at, t4);
    }

    /// Without a race — the normal sequential stale case — the composed
    /// failure must still attach its error to the figures (compose produced
    /// it; the merge must not drop it in favour of the older clean success).
    #[test]
    fn a_sequential_failure_keeps_the_figures_and_attaches_the_new_error() {
        let t1 = Utc::now() - chrono::Duration::seconds(60);
        // The stored success came from the pass with generation 1; the failing
        // pass has generation 2 and preserved the figures (fetched at T1).
        let current = vec![with_fetched_at(ok("codex", 20.0), t1)];
        let generations = gen_map(&[("codex", 1)]);
        let incoming = vec![stale_at("codex", 20.0, t1, "this pass failed")];

        let merged = merge_read_model(&current, &generations, 1, incoming, 2);

        assert_eq!(merged.snapshots[0].windows[0].used_pct, 20.0);
        assert_eq!(
            merged.snapshots[0].error,
            Some(FetchError::Network("this pass failed".into())),
        );
        assert_eq!(merged.snapshots[0].fetched_at, t1);
        assert_eq!(
            merged.generations["codex"], 2,
            "the failing pass is the newest observation"
        );
    }

    /// A figure-less failure from a NEWER pass supersedes an older pass's
    /// figures: the newest attempt observed a provider it never successfully
    /// read, and generation — not completion time — decides. The honest state
    /// is that failure; the next successful pass repopulates the figures.
    #[test]
    fn a_figureless_failure_from_a_newer_pass_supersedes_older_figures() {
        let t3 = Utc::now() - chrono::Duration::seconds(30);
        let t5 = Utc::now() - chrono::Duration::seconds(10);
        for current in [
            with_fetched_at(ok("codex", 20.0), t3),
            stale_at("codex", 20.0, t3, "older failure"),
        ] {
            let generations = gen_map(&[("codex", 1)]);
            let merged = merge_read_model(
                std::slice::from_ref(&current),
                &generations,
                1,
                vec![failed_at("codex", t5, "newer")],
                2,
            );
            assert!(
                merged.snapshots[0].windows.is_empty() && merged.snapshots[0].credits.is_none(),
                "the newest attempt's observation is what the model carries",
            );
            assert_eq!(
                merged.snapshots[0].error,
                Some(FetchError::Network("newer".into()))
            );
            assert_eq!(merged.generations["codex"], 2);
        }
    }

    /// The mirror: a success from an OLDER pass — however late it completed —
    /// cannot modify a figure-less failure from a newer one.
    #[test]
    fn an_older_success_cannot_modify_a_newer_figureless_failure() {
        let t3 = Utc::now() - chrono::Duration::seconds(30);
        let t5 = Utc::now() - chrono::Duration::seconds(10);
        // The store holds a figure-less failure from the newer pass; the older
        // pass's success — slow to persist — merges into it.
        let current = vec![failed_at("codex", t5, "newer failure")];
        let generations = gen_map(&[("codex", 2)]);
        let incoming = vec![with_fetched_at(ok("codex", 20.0), t3)];

        let merged = merge_read_model(&current, &generations, 2, incoming, 1);

        assert!(
            merged.snapshots[0].windows.is_empty() && merged.snapshots[0].credits.is_none(),
            "the causally older success modifies nothing",
        );
        assert_eq!(
            merged.snapshots[0].error,
            Some(FetchError::Network("newer failure".into())),
        );
        assert_eq!(merged.generations["codex"], 2);
    }

    #[test]
    fn a_newer_success_recovers_a_stale_reading() {
        let t2 = Utc::now() - chrono::Duration::seconds(30);
        let t3 = Utc::now() - chrono::Duration::seconds(10);
        let current = vec![stale_at("codex", 20.0, t2, "old failure")];
        let generations = gen_map(&[("codex", 1)]);
        let incoming = vec![with_fetched_at(ok("codex", 30.0), t3)];

        let merged = merge_read_model(&current, &generations, 1, incoming, 2);

        assert_eq!(merged.snapshots[0].windows[0].used_pct, 30.0);
        assert!(
            merged.snapshots[0].error.is_none(),
            "recovered: the error clears"
        );
        assert_eq!(merged.snapshots[0].fetched_at, t3);
    }

    #[test]
    fn an_older_success_does_not_regress_a_newer_stale_reading() {
        let t1 = Utc::now() - chrono::Duration::seconds(60);
        let t4 = Utc::now() - chrono::Duration::seconds(30);
        let current = vec![stale_at("codex", 30.0, t4, "newer failure")];
        let generations = gen_map(&[("codex", 2)]);
        let incoming = vec![with_fetched_at(ok("codex", 20.0), t1)];

        let merged = merge_read_model(&current, &generations, 2, incoming, 1);

        assert_eq!(merged.snapshots[0].windows[0].used_pct, 30.0);
        assert_eq!(
            merged.snapshots[0].error,
            Some(FetchError::Network("newer failure".into())),
        );
        assert_eq!(merged.snapshots[0].fetched_at, t4);
    }

    /// Membership is causal: a newer pass decides which providers the model
    /// contains, so its additions land, its omissions remove, and — the
    /// blocker — an older pass can neither erase a provider a newer pass
    /// stored nor re-add one a newer pass removed, however its own
    /// configuration looked.
    #[test]
    fn membership_decisions_are_authoritative_only_from_newer_passes() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg();
        let now = Utc::now();

        // Pass 1 adds openrouter; its membership decision stands at generation 1.
        SnapshotStore::merge_and_store(
            dir.path(),
            vec![with_fetched_at(ok("openrouter", 10.0), now)],
            1,
            &cfg,
        )
        .unwrap();

        // Pass 2 (newer) no longer contains openrouter: its omission removes it.
        let store = SnapshotStore::merge_and_store(dir.path(), vec![], 2, &cfg).unwrap();
        assert!(store.snapshots.is_empty());
        assert_eq!(store.membership_generation, 2);

        // Pass 1's outcome — an older pass that still saw openrouter — cannot
        // re-add it against pass 2's newer membership decision.
        let store = SnapshotStore::merge_and_store(
            dir.path(),
            vec![with_fetched_at(ok("openrouter", 10.0), now)],
            1,
            &cfg,
        )
        .unwrap();
        assert!(
            store.snapshots.is_empty(),
            "an older pass cannot re-add a provider a newer pass removed",
        );
        assert_eq!(store.membership_generation, 2);
    }

    /// The mirror membership blocker: an older pass that never saw a provider
    /// cannot erase it by omission after a newer pass stored it.
    #[test]
    fn an_older_pass_cannot_erase_a_newer_provider_by_omission() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg();
        let now = Utc::now();

        // Pass 2 stores claude (membership generation 2).
        SnapshotStore::merge_and_store(
            dir.path(),
            vec![with_fetched_at(ok("claude", 25.0), now)],
            2,
            &cfg,
        )
        .unwrap();

        // Pass 1 — began before claude was enabled, so its outcome lacks it —
        // merges last: its omission must not erase the newer provider.
        let store = SnapshotStore::merge_and_store(dir.path(), vec![], 1, &cfg).unwrap();
        assert_eq!(store.snapshots.len(), 1, "the newer provider survives");
        assert_eq!(store.snapshots[0].windows[0].used_pct, 25.0);
        assert_eq!(store.membership_generation, 2, "membership unchanged");
        assert_eq!(store.generations["claude"], 2);
    }

    /// A failed store write falls back to `derive_merged`: the derivation the
    /// open webview is then given must be the causally merged state — an
    /// older pass's raw outcome can never visibly regress the newer model.
    #[test]
    fn derive_merged_never_regresses_the_model_for_a_failed_write() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg();
        let now = Utc::now();

        // A newer pass persisted a clean success (generation 2).
        SnapshotStore::merge_and_store(
            dir.path(),
            vec![with_fetched_at(ok("openrouter", 25.0), now)],
            2,
            &cfg,
        )
        .unwrap();

        // The persist of an older pass (generation 1) failed; its raw outcome
        // — a stale composition from its old prior — is what would have been
        // published unmerged. The fallback derives the merged state instead.
        let fallback = SnapshotStore::derive_merged(
            dir.path(),
            vec![stale_at("openrouter", 10.0, now, "late failure")],
            1,
            &cfg,
        );
        assert_eq!(
            fallback.snapshots[0].windows[0].used_pct, 25.0,
            "the newer success's figures survive the fallback derivation",
        );
        assert!(fallback.snapshots[0].error.is_none());
        assert_eq!(fallback.aggregate.status, Status::Ok);
        assert_eq!(fallback.generations["openrouter"], 2);
        assert_eq!(fallback.membership_generation, 2);
    }

    /// The full race through the store itself: a stale write from a pass that
    /// began later wins per provider — its figures and error are the newest
    /// generation's — while the other provider's newer figures still land,
    /// and the persisted aggregate is recomputed over the merged list.
    #[test]
    fn concurrent_writers_cannot_regress_the_persisted_read_model() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg();
        let t0 = Utc::now() - chrono::Duration::seconds(120);
        let t4 = Utc::now() - chrono::Duration::seconds(60);
        let t5 = Utc::now() - chrono::Duration::seconds(30);

        // Writer 1 — the WorkManager worker: generation 4, both accounts
        // succeeded.
        let worker = vec![
            with_fetched_at(ok("openrouter", 10.0), t4),
            with_fetched_at(ok("claude", 25.0), t4),
        ];
        SnapshotStore::merge_and_store(dir.path(), worker, 4, &cfg).unwrap();

        // Writer 2 — the foreground, generation 5, composed from its stale
        // in-memory prior: openrouter succeeded again, but claude's fetch
        // failed after the worker's and composes as stale from its old T0
        // reading.
        let foreground = vec![
            with_fetched_at(ok("openrouter", 12.0), t5),
            stale_at("claude", 20.0, t0, "late failure"),
        ];
        let store = SnapshotStore::merge_and_store(dir.path(), foreground, 5, &cfg).unwrap();

        assert_eq!(
            store
                .snapshots
                .iter()
                .find(|s| s.provider_id == "openrouter")
                .unwrap()
                .windows[0]
                .used_pct,
            12.0,
            "the newer pass's success lands",
        );
        let claude = store
            .snapshots
            .iter()
            .find(|s| s.provider_id == "claude")
            .unwrap();
        assert_eq!(
            claude.windows[0].used_pct, 20.0,
            "the newest generation's figures win, even when fetched_at is older",
        );
        assert_eq!(
            claude.error,
            Some(FetchError::Network("late failure".into())),
            "the newest generation's failure reason is what the model carries",
        );
        assert_eq!(store.generations["claude"], 5);
        assert_eq!(
            store.aggregate.status,
            Status::Stale,
            "the stored colour reflects the merged list",
        );

        // The mirror: a still-younger pass supersedes again — equal
        // fetched_at, newer generation decides.
        let late_worker = vec![
            with_fetched_at(ok("openrouter", 9.0), t5),
            stale_at("claude", 18.0, t0, "stale worker"),
        ];
        let store = SnapshotStore::merge_and_store(dir.path(), late_worker, 6, &cfg).unwrap();

        assert_eq!(
            store
                .snapshots
                .iter()
                .find(|s| s.provider_id == "openrouter")
                .unwrap()
                .windows[0]
                .used_pct,
            9.0,
            "equal fetched_at, newer generation decides",
        );
        let claude = store
            .snapshots
            .iter()
            .find(|s| s.provider_id == "claude")
            .unwrap();
        assert_eq!(claude.windows[0].used_pct, 18.0);
        assert_eq!(
            claude.error,
            Some(FetchError::Network("stale worker".into())),
        );
        assert_eq!(store.generations["claude"], 6);
    }
}
