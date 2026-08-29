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
    /// regress the model — a fetch that failed after another writer's success
    /// would overwrite newer figures with older, now-stale ones. This closes
    /// that per provider: [`fresher`] decides between the incoming snapshot and
    /// the one currently on disk, so a late partial failure can only ever add
    /// its newer *error*, never erase a newer success's figures
    /// (windows/credits included).
    ///
    /// The read-merge-write runs under an exclusive [`LOCK_NAME`] lock, so
    /// concurrent entries in one process — and the foreground and a widget-only
    /// worker process — serialize here instead of racing the file itself. The
    /// lock is deliberately held only for this merge-and-write (never across a
    /// fetch): it is released on drop, and the kernel releases it outright if a
    /// writer dies holding it, so a crash cannot leave the store stuck.
    ///
    /// The aggregate is recomputed over the merged list — the stored colour must
    /// match the stored cards. The incoming list must already be in configured
    /// display order; the merge substitutes per provider in place and so
    /// preserves it. Providers the incoming pass no longer contains (an account
    /// disabled between passes) keep the whole-replace semantics and drop out.
    pub fn merge_and_store(
        dir: &Path,
        incoming: Vec<UsageSnapshot>,
        cfg: &Config,
    ) -> std::io::Result<Self> {
        let _lock = acquire_store_lock(dir)?;
        let current = Self::load(dir);
        let merged = merge_read_model(current.snapshots, incoming);
        let aggregate = aggregate_status(&merged, cfg);
        let store = Self::from_snapshots(merged, aggregate);
        store.save(dir)?;
        Ok(store)
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

/// Whether a snapshot carries any quota figures at all. A snapshot without
/// figures is a failed fetch that had no prior success to preserve — compose
/// invented nothing for it — so it must never displace figures another writer
/// did manage to persist (see [`fresher`]).
fn has_content(s: &UsageSnapshot) -> bool {
    !s.windows.is_empty() || s.credits.is_some()
}

/// The fresher of two observations of the same provider, deciding which one a
/// merge keeps. This is the rule that makes the persisted read model safe
/// against the writer race (see [`SnapshotStore::merge_and_store`]):
///
/// - **Both carry figures** (success or stale-preserved): the newer
///   `fetched_at` — the figures' own success — wins. A late writer composed
///   from stale prior state loses here, which is the point. At equal times,
///   an errored one outranks a clean one: same figures, but the errored one is
///   the later observation of that success (the sequential stale case — a pass
///   that failed after a success, figures kept, error attached).
/// - **The candidate carries no figures** (a failed first fetch): the
///   candidate's newer *error* is adopted onto the current figures, which are
///   never erased by an observation that produced none.
/// - **Only the current carries no figures** (a writer that knew of no prior
///   success failed while another had succeeded): the mirror — the candidate's
///   figures are kept, and the current's newer failure reason adopted onto
///   them.
/// - **Neither carries figures**: the newer attempt's error wins, ties to the
///   candidate.
fn fresher(current: UsageSnapshot, candidate: UsageSnapshot) -> UsageSnapshot {
    match (has_content(&current), has_content(&candidate)) {
        (true, false) => {
            if candidate.fetched_at > current.fetched_at {
                let mut merged = current;
                merged.error = candidate.error;
                merged
            } else {
                current
            }
        }
        (false, true) => {
            if current.fetched_at > candidate.fetched_at {
                let mut merged = candidate;
                merged.error = current.error;
                merged
            } else {
                candidate
            }
        }
        _ => {
            let current_key = (current.fetched_at, current.error.is_some());
            let candidate_key = (candidate.fetched_at, candidate.error.is_some());
            if candidate_key >= current_key {
                candidate
            } else {
                current
            }
        }
    }
}

/// Merge a freshly composed snapshot list into the current read model, per
/// provider, by [`fresher`]. Incoming order is preserved (callers sort before
/// merging); providers the incoming pass no longer contains keep the
/// whole-replace semantics and drop out — an account disabled between passes
/// must not haunt the read model.
pub fn merge_read_model(
    current: Vec<UsageSnapshot>,
    incoming: Vec<UsageSnapshot>,
) -> Vec<UsageSnapshot> {
    let by_id: HashMap<&str, &UsageSnapshot> = current
        .iter()
        .map(|s| (s.provider_id.as_str(), s))
        .collect();
    incoming
        .into_iter()
        .map(
            |candidate| match by_id.get(candidate.provider_id.as_str()) {
                Some(&current) => fresher(current.clone(), candidate),
                None => candidate,
            },
        )
        .collect()
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
    // same persisted read model. These tests pin the per-provider rule that
    // makes those concurrent writes safe: figures are a generation (their own
    // `fetched_at`), a figure-less failure never erases figures, and the
    // newest error is always adopted. Timestamps are explicit, so every case
    // is deterministic — no sleeps, no clock dependence.

    fn with_fetched_at(mut s: UsageSnapshot, at: DateTime<Utc>) -> UsageSnapshot {
        s.fetched_at = at;
        s
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
    /// time is stamped.
    fn failed_at(id: &str, at: DateTime<Utc>, msg: &str) -> UsageSnapshot {
        let mut s = UsageSnapshot::failed(id, id, FetchError::Network(msg.into()));
        s.fetched_at = at;
        s
    }

    #[test]
    fn a_late_stale_write_cannot_regress_a_newer_success() {
        let t0 = Utc::now() - chrono::Duration::seconds(120);
        let t4 = Utc::now() - chrono::Duration::seconds(30);
        // The worker persisted a success fetched at T4; the foreground —
        // composed from its stale in-memory prior, where codex's figures came
        // from a T0 success — failed this pass and is writing last.
        let current = vec![with_fetched_at(ok("codex", 25.0), t4)];
        let incoming = vec![stale_at("codex", 20.0, t0, "late failure")];

        let merged = merge_read_model(current, incoming);

        assert_eq!(merged.len(), 1);
        assert_eq!(
            merged[0].windows[0].used_pct, 25.0,
            "the newer success's figures survive the late stale write",
        );
        assert!(
            merged[0].error.is_none(),
            "the late stale write adds no error to a newer success",
        );
        assert_eq!(merged[0].fetched_at, t4);
    }

    #[test]
    fn a_sequential_failure_keeps_the_figures_and_attaches_the_new_error() {
        // No race: the on-disk success is the same one compose preserved into
        // the incoming stale snapshot. Equal timestamps, but the errored one is
        // the later observation of that success — it must win, or the error
        // would never reach the persisted model.
        let t0 = Utc::now() - chrono::Duration::seconds(60);
        let current = vec![with_fetched_at(ok("codex", 20.0), t0)];
        let incoming = vec![stale_at("codex", 20.0, t0, "this pass failed")];

        let merged = merge_read_model(current, incoming);

        assert_eq!(merged[0].windows[0].used_pct, 20.0);
        assert_eq!(
            merged[0].error,
            Some(FetchError::Network("this pass failed".into())),
        );
        assert_eq!(merged[0].fetched_at, t0);
    }

    #[test]
    fn a_figureless_failure_never_erases_persisted_figures() {
        let t0 = Utc::now() - chrono::Duration::seconds(120);
        let t5 = Utc::now() - chrono::Duration::seconds(30);
        // A figure-less failure is a first fetch that failed with no prior
        // success in its own writer's view — but another writer did have
        // figures (a clean success, or a stale reading). The figures survive;
        // only the newer error is adopted, whatever the current error was.
        for current in [
            with_fetched_at(ok("codex", 20.0), t0),
            stale_at("codex", 20.0, t0, "older failure"),
        ] {
            let merged = merge_read_model(vec![current], vec![failed_at("codex", t5, "newer")]);
            assert_eq!(
                merged[0].windows[0].used_pct, 20.0,
                "figures survive a figure-less failure",
            );
            assert_eq!(merged[0].fetched_at, t0, "aged by the figures' success");
            assert_eq!(merged[0].error, Some(FetchError::Network("newer".into())));
        }
    }

    #[test]
    fn a_newer_success_recovers_a_stale_reading() {
        let t0 = Utc::now() - chrono::Duration::seconds(120);
        let t4 = Utc::now() - chrono::Duration::seconds(30);
        let current = vec![stale_at("codex", 20.0, t0, "old failure")];
        let incoming = vec![with_fetched_at(ok("codex", 30.0), t4)];

        let merged = merge_read_model(current, incoming);

        assert_eq!(merged[0].windows[0].used_pct, 30.0);
        assert!(merged[0].error.is_none(), "recovered: the error clears");
        assert_eq!(merged[0].fetched_at, t4);
    }

    #[test]
    fn an_older_success_does_not_regress_a_newer_stale_reading() {
        let t0 = Utc::now() - chrono::Duration::seconds(120);
        let t4 = Utc::now() - chrono::Duration::seconds(30);
        // The mirror race: a writer bringing an older success (its fetches
        // started long ago) must not displace the newer stale reading another
        // writer already persisted.
        let current = vec![stale_at("codex", 30.0, t4, "newer failure")];
        let incoming = vec![with_fetched_at(ok("codex", 20.0), t0)];

        let merged = merge_read_model(current, incoming);

        assert_eq!(merged[0].windows[0].used_pct, 30.0);
        assert_eq!(
            merged[0].error,
            Some(FetchError::Network("newer failure".into())),
        );
        assert_eq!(merged[0].fetched_at, t4);
    }

    #[test]
    fn merge_preserves_incoming_order_and_drops_absent_providers() {
        let current = vec![ok("claude", 90.0), ok("codex", 20.0), ok("openrouter", 5.0)];
        // The incoming pass no longer contains openrouter (its account was
        // disabled between passes) and is in its own configured order.
        let incoming = vec![
            with_fetched_at(ok("codex", 21.0), Utc::now()),
            ok("claude", 91.0),
        ];

        let merged = merge_read_model(current, incoming);

        assert_eq!(
            merged
                .iter()
                .map(|s| s.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["codex", "claude"],
            "incoming order preserved; absent providers drop out",
        );
    }

    /// The full race through the store itself, both writer orders: a late
    /// stale write must not regress the newer success per provider, the other
    /// provider's genuine newer figures must still land, and the persisted
    /// aggregate must be recomputed over the merged list (a late stale write
    /// must not recolour the model Stale).
    #[test]
    fn concurrent_writers_cannot_regress_the_persisted_read_model() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg();
        let t0 = Utc::now() - chrono::Duration::seconds(120);
        let t4 = Utc::now() - chrono::Duration::seconds(60);
        let t5 = Utc::now() - chrono::Duration::seconds(30);

        // Writer 1 — the WorkManager worker: composed from fresh priors, both
        // accounts succeeded at T4.
        let worker = vec![
            with_fetched_at(ok("openrouter", 10.0), t4),
            with_fetched_at(ok("claude", 25.0), t4),
        ];
        SnapshotStore::merge_and_store(dir.path(), worker, &cfg).unwrap();

        // Writer 2 — the foreground, composed from its stale in-memory prior:
        // openrouter succeeded again at T5, but claude's fetch failed after
        // the worker's and composes as stale from its old T0 reading.
        let foreground = vec![
            with_fetched_at(ok("openrouter", 12.0), t5),
            stale_at("claude", 20.0, t0, "late failure"),
        ];
        let store = SnapshotStore::merge_and_store(dir.path(), foreground, &cfg).unwrap();

        assert_eq!(
            store
                .snapshots
                .iter()
                .find(|s| s.provider_id == "openrouter")
                .unwrap()
                .windows[0]
                .used_pct,
            12.0,
            "the foreground's genuine newer success still lands",
        );
        let claude = store
            .snapshots
            .iter()
            .find(|s| s.provider_id == "claude")
            .unwrap();
        assert_eq!(
            claude.windows[0].used_pct, 25.0,
            "the worker's newer success survives the late stale write",
        );
        assert!(claude.error.is_none());
        assert_eq!(
            store.aggregate.status,
            Status::Ok,
            "the persisted colour reflects the merged list, not the late stale write",
        );

        // The mirror: the worker's stale write arriving after the foreground's
        // newer figures must not regress them either.
        let late_worker = vec![
            with_fetched_at(ok("openrouter", 9.0), t4),
            stale_at("claude", 18.0, t0, "stale worker"),
        ];
        let store = SnapshotStore::merge_and_store(dir.path(), late_worker, &cfg).unwrap();

        assert_eq!(
            store
                .snapshots
                .iter()
                .find(|s| s.provider_id == "openrouter")
                .unwrap()
                .windows[0]
                .used_pct,
            12.0,
            "an older success does not regress a newer one",
        );
        let claude = store
            .snapshots
            .iter()
            .find(|s| s.provider_id == "claude")
            .unwrap();
        assert_eq!(claude.windows[0].used_pct, 25.0);
        assert!(claude.error.is_none());
        assert_eq!(store.aggregate.status, Status::Ok);
    }
}
