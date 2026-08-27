//! The single shared refresh operation.
//!
//! Every host — the desktop poller today, Android's WorkManager tomorrow —
//! needs the same thing on the same cadence: given shared configuration,
//! host-supplied decrypted credentials, durable spend baselines and prior
//! snapshots, and the alert memory carried since the last poll, fetch every
//! enabled provider and produce ordered snapshots, one aggregate
//! compact-surface status, the alert events that fired, and the outcome of any
//! credential rotation an adapter asked to persist.
//!
//! What is deliberately *not* here: when to run (scheduling) and what to do
//! with the result (drawing a tray icon, posting a notification, presenting a
//! window). Those stay host responsibilities — see `AGENTS.md` and
//! `docs/adr/0006-…` for why a shared poller cannot exist.

use crate::alerts::{AlertEngine, AlertEvent};
use crate::config::Config;
use crate::model::{Status, UsageSnapshot};
use crate::providers::{providers_for, CredentialWrite, ProviderCtx};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

/// The [`Background refresh target`](../../CONTEXT.md) — the interval Android
/// asks the operating system to schedule refresh opportunities at while the app
/// is not in use. It is a best-effort *request*, never a promise a refresh runs
/// at this cadence (WorkManager coalesces, defers under Doze, and honours no
/// exact time), which is why every surface built on it must frame the number as
/// a target, not an "every 15 minutes". The scheduler that actually enqueues
/// the periodic work lives in the host; this constant is the one value it and
/// any copy describing it must agree on.
pub const BACKGROUND_REFRESH_TARGET: Duration = Duration::from_secs(15 * 60);

/// The aggregate contribution every enabled, tray-eligible account folds into:
/// the worst status among them, and the loudest percentage behind it. Compact
/// surfaces (desktop tray icon, Android widget colour) render this directly.
///
/// Serializable because the persisted [`crate::snapshots::SnapshotStore`]
/// carries it alongside the ordered snapshots, so a cold widget can colour
/// itself from the last refresh without recomputing the fold.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AggregateStatus {
    pub status: Status,
    pub pct: f64,
}

impl Default for AggregateStatus {
    fn default() -> Self {
        Self {
            status: Status::Ok,
            pct: 0.0,
        }
    }
}

/// Everything one shared refresh produced. Every later ticket — desktop's
/// existing tray/popup/mini-summary, Android's app and widgets — renders from
/// exactly this, so a host that stops here needs nothing further from
/// quota-core to draw one refresh cycle.
#[derive(Debug, Clone, Default)]
pub struct RefreshOutcome {
    /// Every enabled provider's snapshot, in configured display order
    /// ([`Config::sort_snapshots`]). A snapshot whose fetch failed but which
    /// has an earlier successful reading keeps that reading with the new
    /// error attached, rather than going blank.
    pub snapshots: Vec<UsageSnapshot>,
    pub aggregate: AggregateStatus,
    /// Threshold crossings and low-balance events from this pass, in the order
    /// they were evaluated. Presentation (notify / open a window) is decided
    /// by [`alerts::presentation`], which the host still calls per event.
    pub alerts: Vec<AlertEvent>,
    /// Every credential rotation an adapter asked to persist during this pass,
    /// and whether the host's write succeeded. A failed write is not itself a
    /// fetch error — the snapshot above may still be `Ok` — but the host needs
    /// to know a rotated token did not make it to storage before the process
    /// exits and the rotation is lost.
    pub credential_writes: Vec<CredentialWrite>,
}

/// Fetch every enabled provider and fold the results into one outcome.
///
/// `prior` is the previous pass's snapshots, keyed by provider id — used only
/// to preserve a stale reading across a failed fetch, never mutated here.
/// `engine` carries alert memory (edge-triggered thresholds, baseline state)
/// across calls; the host owns its lifetime for exactly as long as it wants
/// crossings measured against a continuous baseline (process lifetime on
/// desktop; see `AGENTS.md` for why a restart re-baselines).
pub async fn refresh(
    ctx: &ProviderCtx,
    prior: &HashMap<String, UsageSnapshot>,
    engine: &mut AlertEngine,
) -> RefreshOutcome {
    let cfg = &ctx.config;
    let fresh = join_all(
        providers_for(cfg)
            .into_iter()
            .filter(|provider| {
                cfg.providers
                    .get(provider.id())
                    .map(|p| p.enabled)
                    .unwrap_or(false)
            })
            .map(|provider| async move {
                match provider.fetch(ctx).await {
                    Ok(s) => s,
                    Err(e) => UsageSnapshot::failed(provider.id(), provider.name(), e),
                }
            }),
    )
    .await;

    compose(fresh, cfg, prior, engine, ctx.credential_writes())
}

/// The decision core of a refresh pass: everything after the network fetches
/// have already produced snapshots. Pure and synchronous, so it is exercised
/// directly by tests without mocking HTTP — [`refresh`] is a thin async
/// wrapper that supplies the fetched snapshots and the context's collected
/// [`CredentialWrite`]s.
fn compose(
    mut fresh: Vec<UsageSnapshot>,
    cfg: &Config,
    prior: &HashMap<String, UsageSnapshot>,
    engine: &mut AlertEngine,
    credential_writes: Vec<CredentialWrite>,
) -> RefreshOutcome {
    // Stale preservation: a failed fetch with an earlier successful reading on
    // record keeps that reading, with the new error attached, instead of going
    // blank. A snapshot that has never succeeded (no prior, or a prior that was
    // itself empty/errored) has nothing worth preserving.
    for s in &mut fresh {
        if let (Some(err), Some(prev)) = (s.error.clone(), prior.get(&s.provider_id)) {
            if prev.error.is_none() && !(prev.windows.is_empty() && prev.credits.is_none()) {
                let mut merged = prev.clone();
                merged.error = Some(err);
                *s = merged;
            }
        }
    }

    // Display order, applied once so every compact surface agrees.
    cfg.sort_snapshots(&mut fresh);

    // Aggregate compact-surface status folded from the ordered snapshots.
    let aggregate = aggregate_status(&fresh, cfg);

    // Alert memory belongs only to accounts that still exist and are enabled.
    // An account disabled or deleted since the last pass must not keep a
    // remembered level that would suppress a genuine crossing when it returns —
    // re-enabling or recreating starts a fresh baseline (issue #112). Pruning
    // here, in the one place with tests, gives every host that guarantee for
    // free rather than trusting each to forget on every config mutation.
    let enabled: HashSet<String> = cfg
        .providers
        .iter()
        .filter(|(_, p)| p.enabled)
        .map(|(id, _)| id.clone())
        .collect();
    engine.retain_accounts(&enabled);

    // Alerts: edge-triggered in the engine, so this pass only ever reports
    // genuine crossings (or the process's baseline) — never a re-fire for an
    // account sitting still above threshold.
    let mut events = Vec::new();
    for s in &fresh {
        events.extend(engine.evaluate(s, cfg));
    }

    RefreshOutcome {
        snapshots: fresh,
        aggregate,
        alerts: events,
        credential_writes,
    }
}

/// Fold a set of snapshots into the one compact-surface [`AggregateStatus`]:
/// the worst status among tray-eligible accounts and the loudest percentage
/// behind it. Each account contributes the status its tray picker names, unless
/// it opted its icon contribution out via `tray_color`.
///
/// Extracted from [`compose`] so a host that mutates the snapshot list *after*
/// a refresh — Android replaces a lost-rotation or undecryptable-credential
/// snapshot with an explicit failure before persisting (`mobile.rs`) — can
/// recompute the aggregate over the list it is actually about to store, rather
/// than persisting a colour that predates its own replacements.
pub fn aggregate_status(snapshots: &[UsageSnapshot], cfg: &Config) -> AggregateStatus {
    let mut aggregate = AggregateStatus::default();
    for s in snapshots {
        let low = cfg
            .providers
            .get(&s.provider_id)
            .and_then(|p| p.low_balance_warn);
        if cfg.effective_alerts(&s.provider_id).tray_color {
            if let Some((status, pct)) = cfg.mini_tray_status(s, low) {
                aggregate.status = aggregate.status.max(status);
                if let Some(pct) = pct {
                    aggregate.pct = aggregate.pct.max(pct);
                }
            }
        }
    }
    aggregate
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use crate::model::{Credits, FetchError, UsageWindow};

    fn window(id: &str, pct: f64) -> UsageWindow {
        UsageWindow {
            metric_id: id.into(),
            label: id.into(),
            used_pct: pct,
            ..Default::default()
        }
    }

    fn ok(id: &str, pct: f64) -> UsageSnapshot {
        UsageSnapshot::ok(id, id, vec![window("w", pct)], None)
    }

    fn failed(id: &str) -> UsageSnapshot {
        UsageSnapshot::failed(id, id, FetchError::Network("boom".into()))
    }

    fn cfg_with(order: crate::config::SortOrder) -> Config {
        let mut cfg = Config::default();
        for key in ["claude", "codex", "openrouter"] {
            cfg.providers.insert(
                key.into(),
                ProviderConfig {
                    enabled: true,
                    ..Default::default()
                },
            );
        }
        cfg.sort_order = order;
        cfg
    }

    /// One pass exercising every part of the outcome together: ordering, stale
    /// preservation, the aggregate status, an alert crossing, and a credential
    /// write outcome — matching what a real refresh actually produces in one
    /// go, not five isolated units that happen to share a struct.
    #[test]
    fn one_pass_produces_ordering_staleness_aggregate_alerts_and_credential_outcomes() {
        let cfg = cfg_with(crate::config::SortOrder::UsageDesc);
        let mut prior = HashMap::new();
        // codex previously succeeded at a calm reading; this pass's fetch for
        // it fails, so the stale reading should be preserved with the new
        // error attached.
        prior.insert("codex".into(), ok("codex", 20.0));

        let fresh = vec![ok("claude", 90.0), failed("codex"), ok("openrouter", 10.0)];
        let mut engine = AlertEngine::default();
        let credential_writes = vec![
            CredentialWrite {
                key: "claude_oauth".into(),
                value: "rotated-token".into(),
                result: Ok(()),
            },
            CredentialWrite {
                key: "codex_oauth".into(),
                value: "rotated-token".into(),
                result: Err("disk full".into()),
            },
        ];

        let outcome = compose(fresh, &cfg, &prior, &mut engine, credential_writes.clone());

        // Ordering: usage-descending under the default Icon basis.
        assert_eq!(
            outcome
                .snapshots
                .iter()
                .map(|s| s.provider_id.as_str())
                .collect::<Vec<_>>(),
            vec!["claude", "openrouter", "codex"],
        );

        // Staleness: codex kept its 20% reading and carries the new error.
        let codex = outcome
            .snapshots
            .iter()
            .find(|s| s.provider_id == "codex")
            .unwrap();
        assert_eq!(codex.windows[0].used_pct, 20.0);
        assert!(codex.error.is_some());

        // Aggregate: codex's now-errored snapshot contributes Stale, which
        // outranks claude's Warn — a fetch failure must never be masked by a
        // louder but merely-warning account elsewhere in the list. claude's
        // 90% is still the loudest percentage on record.
        assert_eq!(outcome.aggregate.status, Status::Stale);
        assert_eq!(outcome.aggregate.pct, 90.0);

        // Alerts: claude's first-ever poll is a baseline crossing at Warn.
        assert_eq!(outcome.alerts.len(), 1);
        assert_eq!(outcome.alerts[0].provider_id, "claude");
        assert_eq!(outcome.alerts[0].level, crate::alerts::AlertLevel::Warn);

        // Credential writes: passed through verbatim, success and failure
        // alike, so the host can decide what to do about a lost rotation.
        assert_eq!(outcome.credential_writes, credential_writes);
    }

    /// Acceptance #5, through the refresh seam *and* the persistence seam
    /// together: a success is persisted, the process dies, a cold process
    /// reloads it as `prior`, and the next refresh's failed fetch keeps the
    /// persisted figures with the new error attached — marked stale, aged by
    /// the original `fetched_at`, never shown as current. A provider with no
    /// prior success invents nothing on that same cold pass.
    #[test]
    fn stale_reading_survives_a_cold_start_through_the_snapshot_store() {
        use crate::snapshots::SnapshotStore;
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_with(crate::config::SortOrder::Manual);

        // A live process succeeds for codex and persists the read model.
        let first = compose(
            vec![ok("codex", 20.0)],
            &cfg,
            &HashMap::new(),
            &mut AlertEngine::default(),
            vec![],
        );
        SnapshotStore::from_outcome(&first)
            .save(dir.path())
            .unwrap();
        let persisted_fetched_at = first.snapshots[0].fetched_at;

        // The process dies. A cold process reloads the store and refreshes;
        // codex's fetch now fails and claude has never succeeded.
        let reloaded = SnapshotStore::load(dir.path());
        let prior = reloaded.prior_map();
        let second = compose(
            vec![failed("codex"), failed("claude")],
            &cfg,
            &prior,
            &mut AlertEngine::default(),
            vec![],
        );

        let codex = second
            .snapshots
            .iter()
            .find(|s| s.provider_id == "codex")
            .unwrap();
        assert_eq!(codex.windows[0].used_pct, 20.0, "kept the persisted figure");
        assert!(codex.error.is_some(), "carries the new failure");
        assert_eq!(
            codex.fetched_at, persisted_fetched_at,
            "aged by the original success, not re-stamped as current",
        );

        let claude = second
            .snapshots
            .iter()
            .find(|s| s.provider_id == "claude")
            .unwrap();
        assert!(claude.windows.is_empty(), "a first failure invents no data");
        assert!(claude.error.is_some());
    }

    /// A snapshot with no prior successful reading has nothing to preserve, so
    /// a failed first fetch stays visibly failed rather than fabricating data.
    #[test]
    fn a_failed_first_fetch_has_nothing_to_preserve() {
        let cfg = cfg_with(crate::config::SortOrder::Manual);
        let prior = HashMap::new();
        let mut engine = AlertEngine::default();
        let outcome = compose(vec![failed("claude")], &cfg, &prior, &mut engine, vec![]);
        assert!(outcome.snapshots[0].windows.is_empty());
        assert!(outcome.snapshots[0].error.is_some());
    }

    /// A *partial* failure against the persisted read model, through the seams
    /// the hosts actually run (issue #111's stale-reading criterion). One pass
    /// succeeds for two accounts and is persisted; a cold process reloads the
    /// store as `prior` and the next pass fails for codex while succeeding for
    /// claude. The failed account must keep its persisted figures — windows
    /// *and* credits — with the new error attached, aged by its original
    /// `fetched_at`; the succeeded account re-stamps as current. And because
    /// the hosts persist every pass (`from_snapshots` + `save` is the exact
    /// sequence in `mobile.rs::refresh_once` and `widget_jni.rs
    /// ::headless_refresh`), the store written by the failing pass must reload
    /// with precisely that state: that file is what a cold app or widget next
    /// renders.
    #[test]
    fn partial_failure_keeps_persisted_figures_stale_while_the_rest_stays_current() {
        use crate::snapshots::SnapshotStore;
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_with(crate::config::SortOrder::Manual);

        let credits = Credits {
            balance: 42.0,
            label: None,
            unit: "USD".into(),
            used: None,
            granted: None,
            est_tokens_remaining: None,
        };
        let codex_ok = UsageSnapshot::ok("codex", "Codex", vec![window("w", 20.0)], Some(credits));
        let claude_ok = ok("claude", 10.0);
        let first = compose(
            vec![codex_ok, claude_ok],
            &cfg,
            &HashMap::new(),
            &mut AlertEngine::default(),
            vec![],
        );
        SnapshotStore::from_outcome(&first)
            .save(dir.path())
            .unwrap();

        // The process dies. A cold process reloads the persisted store as
        // `prior`; this pass codex's fetch fails, claude's succeeds anew.
        let prior = SnapshotStore::load(dir.path()).prior_map();
        let second = compose(
            vec![failed("codex"), ok("claude", 30.0)],
            &cfg,
            &prior,
            &mut AlertEngine::default(),
            vec![],
        );

        let persisted_codex = prior.get("codex").unwrap();
        let codex = second
            .snapshots
            .iter()
            .find(|s| s.provider_id == "codex")
            .unwrap();
        assert_eq!(
            codex.windows, persisted_codex.windows,
            "kept the persisted window figures",
        );
        assert_eq!(
            codex.credits, persisted_codex.credits,
            "kept the persisted credits",
        );
        assert!(codex.error.is_some(), "carries the new failure");
        assert_eq!(
            codex.fetched_at, persisted_codex.fetched_at,
            "aged by the original success, not re-stamped as current",
        );

        let claude = second
            .snapshots
            .iter()
            .find(|s| s.provider_id == "claude")
            .unwrap();
        assert!(claude.error.is_none());
        assert_eq!(claude.windows[0].used_pct, 30.0);
        assert_ne!(
            claude.fetched_at,
            prior.get("claude").unwrap().fetched_at,
            "a success re-stamps as current",
        );

        // Persist the failing pass the way the hosts do, then read the store
        // back cold: the stale figures must still be there, aged and errored.
        let aggregate = aggregate_status(&second.snapshots, &cfg);
        SnapshotStore::from_snapshots(second.snapshots.clone(), aggregate)
            .save(dir.path())
            .unwrap();
        let third = SnapshotStore::load(dir.path());
        let codex = third
            .snapshots
            .iter()
            .find(|s| s.provider_id == "codex")
            .unwrap();
        assert_eq!(codex.windows, persisted_codex.windows);
        assert_eq!(codex.credits, persisted_codex.credits);
        assert!(codex.error.is_some());
        assert_eq!(codex.fetched_at, persisted_codex.fetched_at);
        let claude = third
            .snapshots
            .iter()
            .find(|s| s.provider_id == "claude")
            .unwrap();
        assert!(claude.error.is_none());
        assert_eq!(claude.windows[0].used_pct, 30.0);
        assert_eq!(
            third.aggregate.status,
            Status::Stale,
            "the persisted aggregate reflects the stale account",
        );
    }

    /// An account that opted out of the icon (`tray_color: false`) never moves
    /// the aggregate, even while critical.
    #[test]
    fn tray_color_opt_out_excludes_an_account_from_the_aggregate() {
        let mut cfg = cfg_with(crate::config::SortOrder::Manual);
        cfg.providers.get_mut("claude").unwrap().alerts = Some(crate::config::AlertToggles {
            toast: true,
            tray_color: false,
            auto_popup: false,
        });
        let mut engine = AlertEngine::default();
        let outcome = compose(
            vec![ok("claude", 99.0)],
            &cfg,
            &HashMap::new(),
            &mut engine,
            vec![],
        );
        assert_eq!(outcome.aggregate.status, Status::Ok);
        assert_eq!(outcome.aggregate.pct, 0.0);
    }

    /// Acceptance #4 through the refresh seam: an account disabled between
    /// passes has its alert memory pruned by `compose`, so when it is
    /// re-enabled its first reading is a fresh baseline rather than a silent
    /// "unchanged" that would swallow a genuine crossing.
    #[test]
    fn disabling_an_account_prunes_its_alert_memory() {
        let mut cfg = cfg_with(crate::config::SortOrder::Manual);
        let mut engine = AlertEngine::default();

        // Pass 1: claude baselines at warn while enabled.
        let out = compose(
            vec![ok("claude", 85.0)],
            &cfg,
            &HashMap::new(),
            &mut engine,
            vec![],
        );
        assert_eq!(out.alerts.len(), 1);
        assert_eq!(out.alerts[0].kind, crate::alerts::AlertKind::Baseline);

        // Pass 2: claude is now disabled (and so not fetched). The prune forgets
        // it even though nothing evaluated it this pass.
        cfg.providers.get_mut("claude").unwrap().enabled = false;
        let out = compose(vec![], &cfg, &HashMap::new(), &mut engine, vec![]);
        assert!(out.alerts.is_empty());

        // Pass 3: re-enabled and read again — a fresh baseline, not silence.
        cfg.providers.get_mut("claude").unwrap().enabled = true;
        let out = compose(
            vec![ok("claude", 85.0)],
            &cfg,
            &HashMap::new(),
            &mut engine,
            vec![],
        );
        assert_eq!(out.alerts.len(), 1);
        assert_eq!(out.alerts[0].kind, crate::alerts::AlertKind::Baseline);
    }

    /// `refresh` itself (the async wrapper) with no enabled providers is a
    /// trivial but real end-to-end pass: no network call is made, and the
    /// outcome is empty rather than an error.
    #[tokio::test]
    async fn refresh_with_no_enabled_providers_is_a_quiet_empty_pass() {
        let dir = tempfile::tempdir().unwrap();
        // quota-core's Config::default() enables claude and codex, which would
        // attempt real network fetches; disable everything for this test.
        let mut cfg = Config::default();
        for p in cfg.providers.values_mut() {
            p.enabled = false;
        }
        let ctx = ProviderCtx::new(dir.path().into(), dir.path().into(), HashMap::new(), cfg);
        let mut engine = AlertEngine::default();
        let outcome = refresh(&ctx, &HashMap::new(), &mut engine).await;
        assert!(outcome.snapshots.is_empty());
        assert!(outcome.alerts.is_empty());
        assert!(outcome.credential_writes.is_empty());
        assert_eq!(outcome.aggregate.status, Status::Ok);
    }
}
