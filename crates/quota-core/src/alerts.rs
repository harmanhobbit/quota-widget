use crate::config::{AlertToggles, Config, Thresholds};
use crate::model::UsageSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// The file the durable alert memory is persisted to, alongside the other
/// per-installation stores in the app config directory. Android's background
/// worker and its foreground app both read and write exactly this path so a
/// crossing measured in one process is not re-fired by the next (issue #112).
const ALERT_MEMORY_FILE: &str = "alert-memory.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertLevel {
    Normal,
    Warn,
    Critical,
}

/// Why an event exists: a threshold the user just crossed, or the state their
/// account was already in when the widget started watching it.
///
/// The distinction is presentation, not severity. Launch is tray-first, so an
/// account that was *already* over its warn threshold must not be allowed to
/// throw the main window at the user before they have asked for anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    /// The metric moved up into this level while the widget was watching.
    Crossing,
    /// The level the metric was already at on the first successful poll.
    Baseline,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AlertEvent {
    pub provider_id: String,
    pub provider_name: String,
    pub level: AlertLevel,
    pub kind: AlertKind,
    /// e.g. "5-hour window at 87%" or "balance low: 1.42 USD"
    pub message: String,
}

/// What the host is allowed to do with an event, once the per-account toggles
/// and the baseline rule are both applied. Deciding this here rather than in
/// the poller keeps the launch policy in the crate that has tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlertPresentation {
    /// Send a desktop notification.
    pub notify: bool,
    /// Present the main window (the auto-open setting).
    pub open_window: bool,
}

/// Launch is tray-first: the tray icon and tooltip are the initial point of
/// access, and nothing the *first* poll discovers may present the main window.
/// A baseline critical state is still worth a notification — being at 97% when
/// you sign in is news — but a baseline warning is not, since the tray colour
/// and tooltip already say so without interrupting anything.
///
/// After the baseline, every event is an ordinary crossing and honours the
/// account's toast and auto-open toggles exactly as before.
pub fn presentation(event: &AlertEvent, toggles: &AlertToggles) -> AlertPresentation {
    match event.kind {
        AlertKind::Crossing => AlertPresentation {
            notify: toggles.toast,
            open_window: toggles.auto_popup,
        },
        AlertKind::Baseline => AlertPresentation {
            notify: toggles.toast && event.level == AlertLevel::Critical,
            open_window: false,
        },
    }
}

/// The text a host posts for an alert, in two forms. `title`/`body` carry the
/// full provider detail shown once the device is unlocked; `public_title`/
/// `public_body` are the generic form Android shows on a **private** lock
/// screen (`Notification.VISIBILITY_PRIVATE`), naming no provider and no figure.
///
/// Producing both here keeps the redaction rule — a provider name or a number
/// is detail, "a provider needs attention" is not — in the crate CI exercises,
/// rather than trusting each host to redact correctly. The native Android host
/// (which owns notification posting per ADR-0006) sets the visibility and the
/// public version from these fields; desktop, whose notifications are never
/// lock-screened, uses `title`/`body` and ignores the public pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationContent {
    pub title: String,
    pub body: String,
    pub public_title: String,
    pub public_body: String,
}

/// Map an [`AlertEvent`] to the [`NotificationContent`] a host posts. The
/// public (lock-screen) form is deliberately identical for every event: it must
/// leak neither which provider nor how close to the limit, only that something
/// is worth unlocking for.
pub fn notification_content(event: &AlertEvent) -> NotificationContent {
    let severity = match event.level {
        AlertLevel::Critical => "critical",
        _ => "warning",
    };
    NotificationContent {
        title: format!("{} — {severity}", event.provider_name),
        body: event.message.clone(),
        public_title: "Quota alert".into(),
        public_body: "Open Quota Widget to view details".into(),
    }
}

/// Whether the host should contextually request the Android 13+ notification
/// permission now (issue #112).
///
/// True only when all three hold: the first successful account has been read
/// (`any_account_succeeded`, so the prompt lands in context rather than at a
/// cold launch), some enabled account actually wants notifications, and the
/// permission has not already been requested. `already_requested` is durable
/// host/platform state — a denial is remembered there — so this returns `false`
/// forever after the one prompt: refresh and widgets keep working either way,
/// and the user is never re-prompted (Settings explains the state instead).
pub fn should_request_notification_permission(
    cfg: &Config,
    any_account_succeeded: bool,
    already_requested: bool,
) -> bool {
    !already_requested
        && any_account_succeeded
        && cfg
            .providers
            .iter()
            .any(|(id, p)| p.enabled && cfg.effective_alerts(id).toast)
}

/// Edge-triggered alert engine: an event fires only when a metric *crosses up*
/// into warn/critical territory, and re-arms once it drops back below (i.e.
/// after the window resets). Polling at 85% every minute produces one alert,
/// not sixty.
///
/// The first successful poll of an account is its *baseline*: current levels
/// are recorded so later polls can be compared against them, and anything
/// already over a threshold is reported as `AlertKind::Baseline` rather than
/// as a fresh crossing. A failed or stale snapshot establishes nothing, so an
/// account whose first fetch errors gets its baseline from the first fetch that
/// actually works.
///
/// ## Durable across the Android lifecycle
///
/// On desktop the engine lives for the process's lifetime and a restart
/// re-baselines. Android is different: separate worker processes, reboot and
/// upgrade would each re-baseline an in-memory engine, re-firing an unchanged
/// warning/critical on every background job. So the engine is *persisted*
/// ([`AlertEngine::save`]) and reloaded ([`AlertEngine::load`]) around each
/// pass, and per ADR-0006 the intact file survives process death, reboot and
/// upgrade. Corruption is treated as derived data (see [`AlertEngine::load`]):
/// discarding it only re-baselines, which is safe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertEngine {
    /// Reserved for forward-compatible migrations, as [`crate::snapshots`] does.
    /// Every field is `#[serde(default)]`, so a newer reader tolerates an older
    /// file and vice versa; only a re-key would need this bumped.
    version: u32,
    /// provider id → window label → last level. Nested rather than a
    /// `(String, String)` key because JSON object keys must be strings — the
    /// tuple key serde_json cannot represent — and because forgetting an
    /// account is then a single `remove` on the outer key.
    window_levels: HashMap<String, HashMap<String, AlertLevel>>,
    balance_low: HashMap<String, bool>,
    /// Accounts whose baseline poll has happened. Keyed per account rather than
    /// held as one global flag, so one provider succeeding does not silently
    /// baseline another whose first fetch is still failing.
    baselined: HashSet<String>,
}

impl Default for AlertEngine {
    fn default() -> Self {
        Self {
            version: 1,
            window_levels: HashMap::new(),
            balance_low: HashMap::new(),
            baselined: HashSet::new(),
        }
    }
}

impl AlertEngine {
    pub fn evaluate(&mut self, snapshot: &UsageSnapshot, cfg: &Config) -> Vec<AlertEvent> {
        let mut events = Vec::new();
        if snapshot.error.is_some() {
            return events; // never alert off stale/failed data
        }
        let kind = if self.baselined.insert(snapshot.provider_id.clone()) {
            AlertKind::Baseline
        } else {
            AlertKind::Crossing
        };
        let Thresholds {
            warn_pct,
            critical_pct,
        } = cfg.effective_thresholds(&snapshot.provider_id);

        for w in snapshot.windows.iter().filter(|w| !w.informational) {
            let level = if w.used_pct >= critical_pct {
                AlertLevel::Critical
            } else if w.used_pct >= warn_pct {
                AlertLevel::Warn
            } else {
                AlertLevel::Normal
            };
            let prev = self
                .window_levels
                .entry(snapshot.provider_id.clone())
                .or_default()
                .insert(w.label.clone(), level)
                .unwrap_or(AlertLevel::Normal);
            if level > prev {
                events.push(AlertEvent {
                    provider_id: snapshot.provider_id.clone(),
                    provider_name: snapshot.provider_name.clone(),
                    level,
                    kind,
                    message: format!("{} window at {:.0}%", w.label, w.used_pct),
                });
            }
        }

        if let Some(credits) = &snapshot.credits {
            if let Some(thr) = cfg
                .providers
                .get(&snapshot.provider_id)
                .and_then(|p| p.low_balance_warn)
            {
                let low = credits.balance <= thr;
                let was_low = self
                    .balance_low
                    .insert(snapshot.provider_id.clone(), low)
                    .unwrap_or(false);
                if low && !was_low {
                    events.push(AlertEvent {
                        provider_id: snapshot.provider_id.clone(),
                        provider_name: snapshot.provider_name.clone(),
                        level: AlertLevel::Warn,
                        kind,
                        message: format!("balance low: {:.2} {}", credits.balance, credits.unit),
                    });
                }
            }
        }

        events
    }

    /// Has at least one account produced a successful reading (established its
    /// baseline)? The host reads this as "the first successful account has
    /// happened" when deciding whether to contextually request the notification
    /// permission — see [`should_request_notification_permission`].
    pub fn has_baseline(&self) -> bool {
        !self.baselined.is_empty()
    }

    /// Drop everything remembered about one account so its next reading is a
    /// fresh baseline. The host calls this the moment an account is disabled or
    /// deleted (issue #112), rather than waiting for the next refresh's prune,
    /// so a background job firing in between cannot suppress the new baseline.
    pub fn forget(&mut self, provider_id: &str) {
        self.window_levels.remove(provider_id);
        self.balance_low.remove(provider_id);
        self.baselined.remove(provider_id);
    }

    /// Keep only the accounts named in `keep`, forgetting every other. Called
    /// once per refresh with the currently-enabled account ids so a disabled or
    /// deleted account's memory cannot outlive it — the durable equivalent of
    /// the in-process engine simply never seeing that account again.
    pub fn retain_accounts(&mut self, keep: &HashSet<String>) {
        self.window_levels.retain(|id, _| keep.contains(id));
        self.balance_low.retain(|id, _| keep.contains(id));
        self.baselined.retain(|id| keep.contains(id));
    }

    /// Load the durable alert memory. A missing, unreadable or malformed file
    /// all read as the empty memory — alert memory is *derived*, exactly like
    /// [`crate::snapshots::SnapshotStore::load`], so a file we cannot parse is
    /// discarded rather than kept-and-blocking the way user-authored config is.
    /// Discarding it only re-baselines every account, which is safe: a baseline
    /// warning is silent and a baseline critical notifies at most once. Per
    /// ADR-0006 the *intact* file is what survives process death, reboot and
    /// upgrade — the whole point of persisting it.
    pub fn load(dir: &Path) -> Self {
        match std::fs::read_to_string(dir.join(ALERT_MEMORY_FILE)) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist the alert memory atomically (temp-file-then-rename), the same
    /// discipline every store in this crate uses so a worker reading it — or a
    /// process killed mid-write — never sees a torn or partial JSON document.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self).expect("alert memory serializes");
        let path = dir.join(ALERT_MEMORY_FILE);
        let tmp = dir.join(format!("{ALERT_MEMORY_FILE}.tmp"));
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Credits, UsageWindow};

    fn snap(pct: f64) -> UsageSnapshot {
        UsageSnapshot::ok(
            "claude",
            "Claude",
            vec![UsageWindow {
                label: "5-hour".into(),
                used_pct: pct,
                ..Default::default()
            }],
            None,
        )
    }

    fn low_balance_snap(balance: f64) -> UsageSnapshot {
        UsageSnapshot::ok(
            "openrouter",
            "OpenRouter",
            vec![],
            Some(Credits {
                balance,
                label: None,
                unit: "USD".into(),
                used: None,
                granted: None,
                est_tokens_remaining: None,
            }),
        )
    }

    fn cfg_with_low_balance() -> Config {
        let mut cfg = Config::default();
        cfg.providers
            .get_mut("openrouter")
            .unwrap()
            .low_balance_warn = Some(5.0);
        cfg
    }

    #[test]
    fn fires_once_per_crossing_and_rearms_on_reset() {
        let cfg = Config::default(); // warn 80, critical 95
        let mut eng = AlertEngine::default();

        assert!(eng.evaluate(&snap(50.0), &cfg).is_empty());
        let ev = eng.evaluate(&snap(85.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].level, AlertLevel::Warn);
        // still at 85 next poll: silent
        assert!(eng.evaluate(&snap(87.0), &cfg).is_empty());
        // escalates to critical: fires again
        let ev = eng.evaluate(&snap(96.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].level, AlertLevel::Critical);
        // window reset: silent, but re-armed
        assert!(eng.evaluate(&snap(2.0), &cfg).is_empty());
        assert_eq!(eng.evaluate(&snap(85.0), &cfg).len(), 1);
    }

    #[test]
    fn jump_straight_to_critical_fires_single_critical() {
        let cfg = Config::default();
        let mut eng = AlertEngine::default();
        // Baseline first, so the jump under test is a genuine crossing.
        assert!(eng.evaluate(&snap(10.0), &cfg).is_empty());
        let ev = eng.evaluate(&snap(99.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].level, AlertLevel::Critical);
        assert_eq!(ev[0].kind, AlertKind::Crossing);
    }

    #[test]
    fn first_poll_records_levels_as_baseline_not_crossings() {
        let cfg = cfg_with_low_balance();
        let mut eng = AlertEngine::default();

        // Already over warn when the widget starts watching.
        let ev = eng.evaluate(&snap(85.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].level, AlertLevel::Warn);
        assert_eq!(ev[0].kind, AlertKind::Baseline);
        // The level was recorded, so staying there stays quiet.
        assert!(eng.evaluate(&snap(87.0), &cfg).is_empty());

        // Already critical on a different account's first poll.
        let mut eng = AlertEngine::default();
        let ev = eng.evaluate(&snap(97.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].level, AlertLevel::Critical);
        assert_eq!(ev[0].kind, AlertKind::Baseline);

        // Already below the low-balance threshold on the first poll.
        let mut eng = AlertEngine::default();
        let ev = eng.evaluate(&low_balance_snap(3.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, AlertKind::Baseline);
        assert!(eng.evaluate(&low_balance_snap(3.0), &cfg).is_empty());
    }

    #[test]
    fn crossings_after_the_baseline_are_ordinary_crossings() {
        let cfg = cfg_with_low_balance();
        let mut eng = AlertEngine::default();

        // A quiet baseline emits nothing at all.
        assert!(eng.evaluate(&snap(10.0), &cfg).is_empty());
        let ev = eng.evaluate(&snap(85.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].level, AlertLevel::Warn);
        assert_eq!(ev[0].kind, AlertKind::Crossing);
        let ev = eng.evaluate(&snap(96.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].level, AlertLevel::Critical);
        assert_eq!(ev[0].kind, AlertKind::Crossing);

        // Low balance crossed after a healthy baseline is also a crossing.
        let mut eng = AlertEngine::default();
        assert!(eng.evaluate(&low_balance_snap(50.0), &cfg).is_empty());
        let ev = eng.evaluate(&low_balance_snap(3.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, AlertKind::Crossing);
    }

    #[test]
    fn a_baseline_warning_reappears_as_a_crossing_after_a_reset() {
        let cfg = Config::default();
        let mut eng = AlertEngine::default();

        assert_eq!(eng.evaluate(&snap(85.0), &cfg)[0].kind, AlertKind::Baseline);
        // Window reset: silent, but re-armed.
        assert!(eng.evaluate(&snap(2.0), &cfg).is_empty());
        let ev = eng.evaluate(&snap(85.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, AlertKind::Crossing);
    }

    #[test]
    fn a_failed_first_poll_establishes_no_baseline() {
        let cfg = Config::default();
        let mut eng = AlertEngine::default();
        let failed = UsageSnapshot::failed(
            "claude",
            "Claude",
            crate::model::FetchError::Network("x".into()),
        );
        assert!(eng.evaluate(&failed, &cfg).is_empty());
        // The first *successful* poll is the baseline, however late it lands.
        let ev = eng.evaluate(&snap(85.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, AlertKind::Baseline);
    }

    #[test]
    fn baseline_never_opens_the_window_and_only_critical_notifies() {
        let toggles = AlertToggles {
            toast: true,
            tray_color: true,
            auto_popup: true, // the setting that must not fire at baseline
        };
        let cfg = cfg_with_low_balance();

        let mut eng = AlertEngine::default();
        let warn = eng.evaluate(&snap(85.0), &cfg).remove(0);
        assert_eq!(
            presentation(&warn, &toggles),
            AlertPresentation {
                notify: false,
                open_window: false
            }
        );

        let mut eng = AlertEngine::default();
        let low = eng.evaluate(&low_balance_snap(3.0), &cfg).remove(0);
        assert_eq!(
            presentation(&low, &toggles),
            AlertPresentation {
                notify: false,
                open_window: false
            }
        );

        let mut eng = AlertEngine::default();
        let critical = eng.evaluate(&snap(97.0), &cfg).remove(0);
        assert_eq!(
            presentation(&critical, &toggles),
            AlertPresentation {
                notify: true,
                open_window: false
            }
        );
    }

    #[test]
    fn post_baseline_events_honour_the_notification_and_auto_open_toggles() {
        let cfg = Config::default();
        let mut eng = AlertEngine::default();
        assert!(eng.evaluate(&snap(10.0), &cfg).is_empty());
        let crossing = eng.evaluate(&snap(85.0), &cfg).remove(0);

        let all_on = AlertToggles {
            toast: true,
            tray_color: true,
            auto_popup: true,
        };
        assert_eq!(
            presentation(&crossing, &all_on),
            AlertPresentation {
                notify: true,
                open_window: true
            }
        );

        let all_off = AlertToggles {
            toast: false,
            tray_color: true,
            auto_popup: false,
        };
        assert_eq!(
            presentation(&crossing, &all_off),
            AlertPresentation {
                notify: false,
                open_window: false
            }
        );

        // A baseline critical still respects a disabled toast toggle.
        let mut eng = AlertEngine::default();
        let baseline_critical = eng.evaluate(&snap(97.0), &cfg).remove(0);
        assert_eq!(
            presentation(&baseline_critical, &all_off),
            AlertPresentation {
                notify: false,
                open_window: false
            }
        );
    }

    #[test]
    fn no_alerts_from_errored_snapshots() {
        let cfg = Config::default();
        let mut eng = AlertEngine::default();
        let s = UsageSnapshot::failed(
            "claude",
            "Claude",
            crate::model::FetchError::Network("x".into()),
        );
        assert!(eng.evaluate(&s, &cfg).is_empty());
    }

    #[test]
    fn low_balance_fires_once() {
        let mut cfg = Config::default();
        cfg.providers
            .get_mut("openrouter")
            .unwrap()
            .low_balance_warn = Some(5.0);
        let mut eng = AlertEngine::default();
        let s = UsageSnapshot::ok(
            "openrouter",
            "OpenRouter",
            vec![],
            Some(Credits {
                balance: 3.0,
                label: None,
                unit: "USD".into(),
                used: None,
                granted: None,
                est_tokens_remaining: None,
            }),
        );
        assert_eq!(eng.evaluate(&s, &cfg).len(), 1);
        assert!(eng.evaluate(&s, &cfg).is_empty());
    }

    // ---- Durable alert memory (issue #112) ---------------------------------

    /// Acceptance #1 + #3, through the persistence seam: an unchanged
    /// warning/critical state does not re-notify when the engine is reloaded
    /// from disk — which is exactly what a fresh worker process, a reboot and
    /// an upgrade each look like — while a genuine crossing fires once.
    #[test]
    fn edge_triggering_survives_reload_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::default(); // warn 80, critical 95

        // First process: baseline at warn, then persist.
        let mut eng = AlertEngine::default();
        assert_eq!(eng.evaluate(&snap(85.0), &cfg)[0].kind, AlertKind::Baseline);
        eng.save(dir.path()).unwrap();

        // Simulated worker recreation: a brand-new engine off disk. The level
        // is remembered, so an unchanged reading stays silent.
        let mut eng = AlertEngine::load(dir.path());
        assert!(eng.evaluate(&snap(87.0), &cfg).is_empty());
        eng.save(dir.path()).unwrap();

        // Reboot-equivalent reload: a genuine escalation to critical fires once.
        let mut eng = AlertEngine::load(dir.path());
        let ev = eng.evaluate(&snap(96.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].level, AlertLevel::Critical);
        assert_eq!(ev[0].kind, AlertKind::Crossing);
        eng.save(dir.path()).unwrap();

        // Upgrade-equivalent reload: still critical, still silent.
        let mut eng = AlertEngine::load(dir.path());
        assert!(eng.evaluate(&snap(97.0), &cfg).is_empty());
    }

    /// Acceptance #2: a first-reading-critical account produces at most one
    /// baseline critical, even across a reload — the reload must not re-baseline
    /// an account whose baseline was already persisted.
    #[test]
    fn baseline_critical_is_recorded_and_not_repeated_after_reload() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::default();

        let mut eng = AlertEngine::default();
        let ev = eng.evaluate(&snap(97.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, AlertKind::Baseline);
        assert_eq!(ev[0].level, AlertLevel::Critical);
        eng.save(dir.path()).unwrap();

        // A new worker reloads the memory: the account is already baselined and
        // its critical level recorded, so the same reading is silent.
        let mut eng = AlertEngine::load(dir.path());
        assert!(eng.has_baseline());
        assert!(eng.evaluate(&snap(97.0), &cfg).is_empty());
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = cfg_with_low_balance();
        let mut eng = AlertEngine::default();
        eng.evaluate(&snap(85.0), &cfg);
        eng.evaluate(&low_balance_snap(3.0), &cfg);
        eng.save(dir.path()).unwrap();
        assert_eq!(AlertEngine::load(dir.path()), eng);
    }

    /// Derived data: a file we cannot parse re-baselines rather than blocking
    /// the next save, matching `crate::snapshots`'s corruption policy.
    #[test]
    fn a_corrupt_file_re_baselines_and_does_not_block_saving() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(ALERT_MEMORY_FILE), "{ not json").unwrap();
        let eng = AlertEngine::load(dir.path());
        assert_eq!(eng, AlertEngine::default());
        assert!(!eng.has_baseline());
        // Saving over the corrupt file just works.
        eng.save(dir.path()).unwrap();
        assert_eq!(AlertEngine::load(dir.path()), AlertEngine::default());
    }

    #[test]
    fn a_missing_file_is_the_empty_memory() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(AlertEngine::load(dir.path()), AlertEngine::default());
    }

    /// Acceptance #4: forgetting an account clears its memory so re-reading it
    /// starts a new baseline — a baseline critical may notify once more.
    #[test]
    fn forget_clears_one_accounts_memory_and_leaves_others() {
        let cfg = Config::default();
        let mut eng = AlertEngine::default();
        // claude and a second account both baseline at critical.
        assert_eq!(eng.evaluate(&snap(97.0), &cfg)[0].kind, AlertKind::Baseline);
        let other = UsageSnapshot::ok(
            "codex",
            "Codex",
            vec![UsageWindow {
                label: "5-hour".into(),
                used_pct: 96.0,
                ..Default::default()
            }],
            None,
        );
        assert_eq!(eng.evaluate(&other, &cfg)[0].kind, AlertKind::Baseline);

        eng.forget("claude");

        // claude re-baselines (one more baseline critical is allowed)...
        let ev = eng.evaluate(&snap(97.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, AlertKind::Baseline);
        // ...while codex, untouched, stays silent at its unchanged level.
        assert!(eng.evaluate(&other, &cfg).is_empty());
    }

    /// Acceptance #4, the automatic path: `retain_accounts` (called each refresh
    /// with the enabled ids) forgets an account that has since been disabled or
    /// deleted, so it re-baselines when it returns.
    #[test]
    fn retain_accounts_forgets_absent_accounts() {
        let cfg = Config::default();
        let mut eng = AlertEngine::default();
        eng.evaluate(&snap(85.0), &cfg); // claude baselined at warn

        // Only "codex" survives a config that no longer enables claude.
        let keep: HashSet<String> = ["codex".to_string()].into_iter().collect();
        eng.retain_accounts(&keep);
        assert!(!eng.has_baseline());

        // claude re-enabled: its first reading is a fresh baseline again.
        assert_eq!(eng.evaluate(&snap(85.0), &cfg)[0].kind, AlertKind::Baseline);
    }

    // ---- Notification content & permission decision (issue #112) -----------

    #[test]
    fn notification_content_keeps_detail_off_the_lock_screen() {
        let cfg = Config::default();
        let mut eng = AlertEngine::default();
        eng.evaluate(&snap(10.0), &cfg); // quiet baseline
        let ev = eng.evaluate(&snap(96.0), &cfg).remove(0);
        let c = notification_content(&ev);

        // Full form names the provider and the figure.
        assert!(c.title.contains("Claude"));
        assert!(c.title.contains("critical"));
        assert!(c.body.contains("96%"));

        // Public (lock-screen) form leaks neither the provider nor any digit.
        assert!(!c.public_title.contains("Claude"));
        assert!(!c.public_body.contains("Claude"));
        assert!(!c.public_body.chars().any(|ch| ch.is_ascii_digit()));
        // ...and is identical regardless of severity.
        let mut eng = AlertEngine::default();
        let warn = notification_content(&eng.evaluate(&snap(85.0), &cfg).remove(0));
        assert_eq!(warn.public_title, c.public_title);
        assert_eq!(warn.public_body, c.public_body);
    }

    #[test]
    fn permission_is_requested_once_after_a_success_with_notifications_on() {
        let cfg = Config::default(); // enables claude + codex, toast on by default

        // Not before any account has succeeded, even with notifications on.
        assert!(!should_request_notification_permission(&cfg, false, false));
        // After the first success, with notifications wanted and not yet asked.
        assert!(should_request_notification_permission(&cfg, true, false));
        // Never twice: a prior request (grant or denial) suppresses it forever.
        assert!(!should_request_notification_permission(&cfg, true, true));

        // Not when no enabled account wants notifications.
        let mut quiet = Config::default();
        for p in quiet.providers.values_mut() {
            p.alerts = Some(AlertToggles {
                toast: false,
                tray_color: true,
                auto_popup: false,
            });
        }
        assert!(!should_request_notification_permission(&quiet, true, false));
    }
}
