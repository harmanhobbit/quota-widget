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
}

impl Default for SnapshotStore {
    fn default() -> Self {
        Self {
            version: 1,
            refreshed_at: None,
            aggregate: AggregateStatus::default(),
            snapshots: Vec::new(),
            generations: HashMap::new(),
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
        // The membership authority inside derive_merged reads the
        // configuration under this same lock, and `Config::save` — the only
        // way configuration reaches disk — takes it too. The whole
        // read-merge-validate-write is therefore atomic against configuration
        // writes: no window exists between the authority comparison and the
        // save for ANY writer, coordinated or not.
        let store = Self::derive_merged(dir, incoming, attempt, cfg);
        store.save(dir)?;
        Ok(store)
    }

    /// Whether `cfg` — the configuration a pass observed — still matches the
    /// configuration currently on disk. Only then may the pass decide
    /// membership: its outcome was composed from that configuration, so if it
    /// has since changed, its additions and omissions describe a world that no
    /// longer exists and must not erase or re-add providers.
    ///
    /// Called inside the store lock, twice in [`merge_and_store`] — once to
    /// decide, once to re-validate immediately before the save — so an
    /// uncoordinated configuration writer that changed the file mid-merge is
    /// detected. Writers that coordinate take [`store_lock`] around their
    /// write, making the interleave impossible in the first place.
    fn membership_authority(dir: &Path, cfg: &Config) -> bool {
        enabled_ids(cfg) == enabled_ids(&Config::load(dir).config)
    }

    /// The causally merged read model for `incoming`, computed against the
    /// store as it currently reads — **without writing**. The caller MUST hold
    /// [`store_lock`] across this call and across whatever publication of the
    /// result follows: the membership authority inside reads the
    /// configuration, and holding the lock makes that observation, the merge
    /// and the publication one atomic stretch against configuration writes
    /// (which all take the same lock through `Config::save`). The locked
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
        let authoritative = Self::membership_authority(dir, cfg);
        let merged = merge_read_model(
            &current.snapshots,
            &current.generations,
            authoritative,
            incoming,
            attempt,
        );
        let mut snapshots = merged.snapshots;
        cfg.sort_snapshots(&mut snapshots);
        let aggregate = aggregate_status(&snapshots, cfg);
        let mut store = Self::from_snapshots(snapshots, aggregate);
        store.generations = merged.generations;
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
    // Exhaustion must fail explicitly: reusing u64::MAX would hand a new pass
    // an equal generation, and equal generations keep the stored snapshot —
    // the exhausted allocator's results would silently never apply.
    let next = last
        .checked_add(1)
        .ok_or_else(|| std::io::Error::other("attempt generation counter exhausted"))?;
    // Temp-then-rename: a torn counter write must not rewind generations.
    let tmp = dir.join(format!("{GENERATION_NAME}.tmp"));
    std::fs::write(&tmp, next.to_string())?;
    std::fs::rename(tmp, dir.join(GENERATION_NAME))?;
    Ok(next)
}

/// The publication gate: admits a generation for delivery to the in-memory
/// state and the open webview only if it is strictly newer than everything
/// already admitted. The durable merge (under the store lock) decides the
/// order in which results reach disk; this gate makes the *publication* order
/// agree with it, so a slow older pass — or a failed-persist fallback —
/// cannot regress cards a newer generation already delivered.
///
/// [`publish`] is the protocol: admit and publish in one critical section,
/// under the caller's mutex. Admitting and then releasing before the result
/// reaches memory/the webview would reopen the race — gen1 admitted and
/// paused, gen2 publishing fully, gen1 resuming and regressing what gen2
/// delivered — so the callback runs while the gate still holds its verdict.
#[derive(Debug)]
pub struct PublicationGate {
    last: u64,
}

impl PublicationGate {
    pub const fn new(last: u64) -> Self {
        Self { last }
    }

    /// Publish `generation` through `publish` iff strictly newer than
    /// everything already published, and return its result. The callback runs
    /// while the gate's decision is held — publication and admission are one
    /// critical section, so an equal or older generation can never interleave
    /// between a newer one's admission and its delivery. A refused generation
    /// returns `None` and runs nothing.
    pub fn publish(&mut self, generation: u64, publish: impl FnOnce()) -> bool {
        if generation > self.last {
            self.last = generation;
            publish();
            true
        } else {
            false
        }
    }
}

/// Hold the store lock for a coordinated writer outside this module —
/// `set_config`'s configuration save. The membership-authority comparison in
/// [`merge_and_store`] reads the configuration under this same lock, so a
/// coordinated writer cannot land between the comparison and the merge's own
/// save; the guard is held only for the write, and the kernel releases it if
/// the writer dies.
pub fn store_lock(dir: &Path) -> std::io::Result<std::fs::File> {
    acquire_store_lock(dir)
}

/// Whether a snapshot carries any quota figures at all. A snapshot without
/// figures is a failed fetch that had no prior success in its own view — it
/// has nothing to contribute visually and must never blank figures another
/// observation carried (see [`fresher`]).
fn has_content(s: &UsageSnapshot) -> bool {
    !s.windows.is_empty() || s.credits.is_some()
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

/// The ids a configuration would fetch: every enabled provider. Two
/// configurations with equal id sets produce the same membership decisions,
/// even if unrelated settings differ.
fn enabled_ids(cfg: &Config) -> std::collections::BTreeSet<String> {
    cfg.providers
        .iter()
        .filter(|(_, p)| p.enabled)
        .map(|(id, _)| id.clone())
        .collect()
}

/// The result of [`merge_read_model`]: the merged snapshots, and the
/// generation each kept one now carries.
pub struct MergedReadModel {
    pub snapshots: Vec<UsageSnapshot>,
    pub generations: HashMap<String, u64>,
}

/// The fresher of two observations of the same provider, deciding which one a
/// merge keeps. The **attempt generation** — allocated from the persisted
/// monotonic counter before the pass's first fetch — orders the two halves,
/// with one invariant layered on top: *last successful readings are never
/// erased*.
///
/// - **Figures** come from the newest attempt that actually carries them. A
///   figure-less failure — a first fetch that failed with no prior success in
///   its own view — has nothing to contribute and must not blank a successful
///   reading, however recent it is; between figure-bearing observations the
///   younger generation wins. `fetched_at` is display metadata of the winning
///   figures' own success (ageing the card); it never decides the race.
/// - **The failure reason** comes from the newest attempt outright: a newer
///   clean success clears the error (recovery), a newer failure replaces it.
/// - The kept generation is the newer of the two — the newest observation of
///   the provider, figures or not.
fn fresher(
    current: &UsageSnapshot,
    current_attempt: u64,
    candidate: &UsageSnapshot,
    candidate_attempt: u64,
) -> UsageSnapshot {
    match candidate_attempt.cmp(&current_attempt) {
        // The newer attempt wins — but a figure-less failure never blanks the
        // stored figures: its failure reason is adopted onto them instead.
        std::cmp::Ordering::Greater => {
            if has_content(candidate) {
                candidate.clone()
            } else if has_content(current) {
                let mut merged = current.clone();
                merged.error = candidate.error.clone();
                merged
            } else {
                candidate.clone()
            }
        }
        // The stored snapshot stands — unless it is figure-less and the older
        // candidate carried figures: last successful readings are never
        // erased, and the stored failure's reason is kept.
        std::cmp::Ordering::Less => {
            if has_content(current) {
                current.clone()
            } else if has_content(candidate) {
                let mut merged = candidate.clone();
                merged.error = current.error.clone();
                merged
            } else {
                current.clone()
            }
        }
        // Equal generations cannot be produced by [`next_generation`]; should
        // one appear anyway, the stored snapshot stands wholesale.
        std::cmp::Ordering::Equal => current.clone(),
    }
}

/// Merge a freshly composed snapshot list into the current read model, per
/// provider, by [`fresher`] — and decide membership. A provider the incoming
/// pass contains but the model does not is an *addition*; a provider the model
/// contains but the pass does not is a *removal*. Both are membership
/// decisions, and the caller marks them authoritative only when the pass's
/// observed configuration still matches the current one — an older pass's
/// outcome was composed from an older configuration, and letting it decide
/// membership would erase a provider a newer pass stored, or re-add one a
/// newer pass removed. Per-provider freshness still applies within that rule:
/// an older pass's fresher figures for a provider that remains in the model do
/// land.
///
/// Incoming order is preserved for the providers it contains; when the pass is
/// not authoritative, providers it could not see are appended as stored.
pub fn merge_read_model(
    current: &[UsageSnapshot],
    current_generations: &HashMap<String, u64>,
    authoritative: bool,
    incoming: Vec<UsageSnapshot>,
    attempt: u64,
) -> MergedReadModel {
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
            // for a pass whose configuration still matches the current one.
            None if authoritative => {
                generations.insert(id.clone(), attempt);
                merged.push(candidate);
            }
            // An older configuration cannot re-add what a newer one removed.
            None => {}
        }
    }
    if authoritative {
        // The pass's configuration matches the current one: providers it no
        // longer contains drop out, with their generations.
        generations.retain(|id, _| merged.iter().any(|s| &s.provider_id == id));
    } else {
        // The pass's configuration is stale: providers it could not see stay
        // exactly as stored.
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
        cfg_enabled(&["openrouter", "claude"])
    }

    /// A configuration with exactly `enabled` enabled providers — the
    /// membership-authority comparisons distinguish configurations by their
    /// enabled sets.
    fn cfg_enabled(enabled: &[&str]) -> Config {
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
            true,
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
            true,
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
        // The observed configuration is saved before the pass runs, as
        // mobile.rs's setup does on Android.
        cfg.save(dir.path()).unwrap();
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
        cfg.save(dir.path()).unwrap();
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
        cfg.save(dir.path()).unwrap();
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
        cfg.save(dir.path()).unwrap();
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

        let merged = merge_read_model(&current, &generations, true, incoming, 1);

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

        let merged = merge_read_model(&current, &generations, true, incoming, 2);

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
    /// THE regression test for the failure-retention contract: a newer pass
    /// whose fetch failed figure-less retains the persisted successful
    /// figures — it can attach its error, but never blank the readings. Holds
    /// for a clean prior and for a prior that was itself stale.
    #[test]
    fn a_newer_figureless_failure_retains_persisted_figures() {
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
                true,
                vec![failed_at("codex", t5, "newer")],
                2,
            );
            assert_eq!(
                merged.snapshots[0].windows[0].used_pct, 20.0,
                "the newer figure-less failure retains the successful figures",
            );
            assert_eq!(
                merged.snapshots[0].fetched_at, t3,
                "aged by the figures' success"
            );
            assert_eq!(
                merged.snapshots[0].error,
                Some(FetchError::Network("newer".into())),
                "its newer failure reason is adopted",
            );
            assert_eq!(merged.generations["codex"], 2);
        }
    }

    /// The mirror: figures from an OLDER successful pass survive under a
    /// NEWER figure-less failure — the readings are never erased, whatever
    /// order the writers land in.
    #[test]
    fn an_older_success_keeps_its_figures_under_a_newer_figureless_failure() {
        let t3 = Utc::now() - chrono::Duration::seconds(30);
        let t5 = Utc::now() - chrono::Duration::seconds(10);
        // The store holds a figure-less failure from the newer pass; the older
        // pass's success — slow to persist — merges into it.
        let current = vec![failed_at("codex", t5, "newer failure")];
        let generations = gen_map(&[("codex", 2)]);
        let incoming = vec![with_fetched_at(ok("codex", 20.0), t3)];

        let merged = merge_read_model(&current, &generations, true, incoming, 1);

        assert_eq!(
            merged.snapshots[0].windows[0].used_pct, 20.0,
            "the older success's figures are rescued",
        );
        assert_eq!(merged.snapshots[0].fetched_at, t3);
        assert_eq!(
            merged.snapshots[0].error,
            Some(FetchError::Network("newer failure".into())),
            "the newer attempt's failure reason is what the model carries",
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

        let merged = merge_read_model(&current, &generations, true, incoming, 2);

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

        let merged = merge_read_model(&current, &generations, true, incoming, 1);

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
    /// Membership authority is decided against the **current configuration**:
    /// a pass whose observed enabled-provider set still matches it may add and
    /// remove; a pass whose configuration has since changed may not — its
    /// additions and omissions describe a world that no longer exists.
    #[test]
    fn membership_decisions_are_authoritative_only_against_the_current_configuration() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();

        // Pass 1 (config: openrouter + claude) adds both.
        let cfg = cfg();
        cfg.save(dir.path()).unwrap();
        SnapshotStore::merge_and_store(
            dir.path(),
            vec![
                with_fetched_at(ok("openrouter", 10.0), now),
                with_fetched_at(ok("claude", 25.0), now),
            ],
            1,
            &cfg,
        )
        .unwrap();

        // The configuration changes: claude is disabled, and the change is
        // saved — what pass 2 will observe.
        let current_cfg = cfg_enabled(&["openrouter"]);
        current_cfg.save(dir.path()).unwrap();

        // Pass 2 observes the NEW configuration and omits claude: authoritative.
        let store = SnapshotStore::merge_and_store(
            dir.path(),
            vec![with_fetched_at(ok("openrouter", 12.0), now)],
            2,
            &current_cfg,
        )
        .unwrap();
        assert_eq!(
            store
                .snapshots
                .iter()
                .map(|s| s.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["openrouter"],
            "the newer pass's removal lands",
        );

        // Pass 1's outcome — an older pass that still observed claude — cannot
        // re-add it against the changed configuration. Its observed
        // configuration is the pre-change one, which no longer matches what
        // is on disk.
        let stale_pass = vec![
            with_fetched_at(ok("openrouter", 9.0), now),
            with_fetched_at(ok("claude", 25.0), now),
        ];
        let store = SnapshotStore::merge_and_store(dir.path(), stale_pass, 1, &cfg).unwrap();
        assert_eq!(
            store
                .snapshots
                .iter()
                .map(|s| s.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["openrouter"],
            "an older pass cannot re-add a provider the current configuration removed",
        );
        assert_eq!(
            store.snapshots[0].windows[0].used_pct, 12.0,
            "its fresher openrouter data still lands"
        );
    }

    /// The mirror membership blocker: an older pass that never saw a provider
    /// cannot erase it by omission after a newer pass stored it — its
    /// configuration no longer matches the current one.
    #[test]
    fn an_older_pass_cannot_erase_a_newer_provider_by_omission() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();

        // The configuration gains claude, and pass 2 (observing it) stores it.
        let wider_cfg = cfg();
        wider_cfg.save(dir.path()).unwrap();
        SnapshotStore::merge_and_store(
            dir.path(),
            vec![with_fetched_at(ok("claude", 25.0), now)],
            2,
            &wider_cfg,
        )
        .unwrap();

        // The configuration changes back (claude disabled) before the older
        // pass merges — but only in the older pass's own observation: the
        // configuration ON DISK still has claude, which is what the merge
        // compares against.
        let narrower_cfg = cfg_enabled(&["openrouter"]);
        let store = SnapshotStore::merge_and_store(dir.path(), vec![], 1, &narrower_cfg).unwrap();
        assert_eq!(store.snapshots.len(), 1, "the newer provider survives");
        assert_eq!(store.snapshots[0].windows[0].used_pct, 25.0);
        assert_eq!(store.generations["claude"], 2);
    }

    /// The membership-authority probe flips when the configuration changes
    /// under the merge: a pass validated against configuration A must not
    /// finalize membership once B is the configuration on disk. This is the
    /// detection primitive behind merge_and_store's re-validation.
    #[test]
    fn config_write_between_comparison_and_save_flips_authority() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_a = cfg();
        cfg_a.save(dir.path()).unwrap();
        assert!(
            SnapshotStore::membership_authority(dir.path(), &cfg_a),
            "the pass's configuration matches the current one"
        );

        // A configuration write lands (coordinated or not) while the pass is
        // merging: claude is disabled.
        let cfg_b = cfg_enabled(&["openrouter"]);
        cfg_b.save(dir.path()).unwrap();

        assert!(
            !SnapshotStore::membership_authority(dir.path(), &cfg_a),
            "the re-validation sees the changed configuration and refuses authority"
        );
        assert!(
            SnapshotStore::membership_authority(dir.path(), &cfg_b),
            "a pass that observed the new configuration is authoritative"
        );
    }

    /// Configuration persistence coordinates through the store lock: while a
    /// merge or fallback derivation holds it (its membership check and save
    /// live inside that window), a configuration write — which takes the same
    /// lock inside `Config::save` — cannot land. THE regression for the
    /// config-write-between-comparison-and-save race.
    #[test]
    fn configuration_persistence_coordinates_through_the_store_lock() {
        use std::sync::mpsc;

        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg();
        cfg.save(dir.path()).unwrap();

        // The merge (or fallback derivation) holds the store lock across its
        // authority comparison, merge, and save.
        let guard = store_lock(dir.path()).unwrap();

        // A configuration write starts while that window is held: `Config::
        // save` takes the same lock, so it must block until the window closes.
        let dir_path = dir.path().to_path_buf();
        let (write_done_tx, write_done_rx) = mpsc::channel::<()>();
        let _writer = std::thread::spawn(move || {
            let changed = cfg_enabled(&["openrouter"]);
            changed.save(&dir_path).unwrap();
            let _ = write_done_tx.send(());
        });

        // Give the writer a fair chance to (wrongly) land inside the window.
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            write_done_rx.try_recv().is_err(),
            "a configuration write must not land while the merge holds the store lock"
        );

        // Closing the window lets the write through.
        drop(guard);
        write_done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the configuration write completes once the lock is released");
    }

    /// The fallback's derivation is atomic against configuration writes: with
    /// the store lock held across the derivation (as mobile.rs's
    /// persist-failure path does), a concurrent write blocks; the derivation
    /// evaluates its membership authority against the configuration as of its
    /// own locked window, and a write landing after it is honored only by the
    /// NEXT pass.
    #[test]
    fn the_fallback_derivation_is_atomic_against_a_configuration_write() {
        use std::sync::mpsc;

        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();

        // A newer pass (generation 2) stored openrouter and claude, observing
        // the configuration with both enabled.
        let cfg_wide = cfg();
        cfg_wide.save(dir.path()).unwrap();
        SnapshotStore::merge_and_store(
            dir.path(),
            vec![
                with_fetched_at(ok("openrouter", 12.0), now),
                with_fetched_at(ok("claude", 25.0), now),
            ],
            2,
            &cfg_wide,
        )
        .unwrap();

        // The fallback for an older pass (generation 1, same observed
        // configuration) takes the store lock across its derivation.
        let guard = store_lock(dir.path()).unwrap();

        // A configuration write (claude disabled) attempts to land
        // concurrently: it must block — the derivation's membership check and
        // its result are one atomic stretch against it.
        let dir_path = dir.path().to_path_buf();
        let (write_done_tx, write_done_rx) = mpsc::channel::<()>();
        let _writer = std::thread::spawn(move || {
            let cfg_narrow = cfg_enabled(&["openrouter"]);
            cfg_narrow.save(&dir_path).unwrap();
            let _ = write_done_tx.send(());
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            write_done_rx.try_recv().is_err(),
            "a configuration write cannot land during the fallback's locked derivation"
        );

        // The derivation: the older pass's figures lose per provider to the
        // newer generation, and the membership stays consistent with the
        // configuration as of the locked window (both providers present).
        let merged = SnapshotStore::derive_merged(
            dir.path(),
            vec![
                with_fetched_at(ok("openrouter", 9.0), now),
                with_fetched_at(ok("claude", 18.0), now),
            ],
            1,
            &cfg_wide,
        );
        assert_eq!(
            merged
                .snapshots
                .iter()
                .find(|s| s.provider_id == "openrouter")
                .unwrap()
                .windows[0]
                .used_pct,
            12.0,
            "the newer generation's figures win per provider",
        );
        assert!(
            merged.snapshots.iter().any(|s| s.provider_id == "claude"),
            "membership is consistent with the configuration as of the locked window",
        );

        // Release the window: the write lands, and a newer pass observing it
        // removes claude by its own authoritative decision.
        drop(guard);
        write_done_rx.recv().unwrap();
        let cfg_narrow = cfg_enabled(&["openrouter"]);
        let store = SnapshotStore::merge_and_store(
            dir.path(),
            vec![with_fetched_at(ok("openrouter", 14.0), now)],
            3,
            &cfg_narrow,
        )
        .unwrap();
        assert_eq!(
            store
                .snapshots
                .iter()
                .map(|s| s.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["openrouter"],
            "the post-write pass's membership decision is authoritative",
        );
    }

    /// The publication gate admits only strictly newer generations: an
    /// out-of-order publication — a slow older pass emitting after a newer
    /// one's store+publish, or a failed-persist fallback racing a newer
    /// writer's commit — is refused, so the cards can never regress.
    #[test]
    fn publication_gate_admits_only_strictly_newer_generations() {
        let mut gate = PublicationGate::new(0);
        assert!(gate.publish(1, || {}), "the first generation publishes");
        assert!(gate.publish(2, || {}), "a newer generation publishes");
        assert!(
            !gate.publish(1, || unreachable!(
                "an older pass's publication must not run after a newer one"
            )),
            "an older pass publishing after a newer one is refused"
        );
        assert!(
            !gate.publish(2, || unreachable!(
                "an equal generation must not re-publish"
            )),
            "an equal generation is refused — already published"
        );
        assert!(
            gate.publish(3, || {}),
            "the next newer generation publishes"
        );
    }

    /// THE interleaving regression for the publication critical section: gen1
    /// is admitted but PAUSES mid-publication; gen2 arrives, must block on the
    /// section, and delivers only after gen1 has finished. A sequential
    /// admission test cannot see this — the section, not the verdict alone,
    /// is what orders concurrent publishers.
    #[test]
    fn publication_critical_section_orders_concurrent_publishers() {
        use std::sync::mpsc;
        use std::sync::{Arc, Mutex as StdMutex};

        let gate = Arc::new(std::sync::Mutex::new(PublicationGate::new(0)));
        let order: Arc<StdMutex<Vec<&'static str>>> = Arc::new(StdMutex::new(Vec::new()));
        let (gen1_started_tx, _gen1_started_rx) = mpsc::channel::<()>();
        let (gen1_resume_tx, gen1_resume_rx) = mpsc::channel::<()>();

        // gen1: admitted, publication paused inside the critical section.
        let gate1 = Arc::clone(&gate);
        let order1 = Arc::clone(&order);
        let gen1 = std::thread::spawn(move || {
            let mut gate = gate1.lock().unwrap();
            gate.publish(1, || {
                order1.lock().unwrap().push("gen1 started");
                gen1_started_tx.send(()).unwrap();
                gen1_resume_rx.recv().unwrap();
                order1.lock().unwrap().push("gen1 delivered");
            })
        });

        // Wait until gen1 is inside its critical section.
        while !order.lock().unwrap().contains(&"gen1 started") {}

        // gen2 (newer) attempts to publish while gen1 is paused: it must BLOCK
        // on the section — not deliver first, not run concurrently.
        let gate2 = Arc::clone(&gate);
        let order2 = Arc::clone(&order);
        let (gen2_done_tx, gen2_done_rx) = mpsc::channel::<bool>();
        let _gen2 = std::thread::spawn(move || {
            let mut gate = gate2.lock().unwrap();
            let published = gate.publish(2, || order2.lock().unwrap().push("gen2 delivered"));
            let _ = gen2_done_tx.send(published);
        });

        // Give gen2 a fair chance to (wrongly) complete while gen1 is paused.
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            gen2_done_rx.try_recv().is_err(),
            "gen2 must not complete while gen1's publication is in flight"
        );

        // Resume gen1: its delivery completes inside the section, then gen2
        // publishes after it.
        gen1_resume_tx.send(()).unwrap();
        assert!(gen1.join().unwrap(), "gen1 publishes");
        assert!(
            gen2_done_rx.recv().unwrap(),
            "gen2 publishes once the section is free"
        );
        assert_eq!(
            *order.lock().unwrap(),
            vec!["gen1 started", "gen1 delivered", "gen2 delivered"],
            "publication order follows admission, never emit timing",
        );
    }

    /// The allocator fails explicitly at exhaustion instead of reusing
    /// u64::MAX: a reused generation would be equal to the stored one, and
    /// equal generations keep the stored snapshot — the exhausted allocator's
    /// results would silently never apply.
    #[test]
    fn generation_allocation_fails_at_exhaustion() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("snapshots.json.gen"), u64::MAX.to_string()).unwrap();
        let err = next_generation(dir.path()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Other);
        assert!(
            err.to_string().contains("exhausted"),
            "the failure names the exhaustion, not a generic io error"
        );
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
        cfg.save(dir.path()).unwrap();
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

        // The observed configuration is saved before the passes run.
        cfg.save(dir.path()).unwrap();

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
