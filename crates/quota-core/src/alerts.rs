use crate::config::{AlertToggles, Config, Thresholds};
use crate::model::UsageSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

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
#[derive(Default)]
pub struct AlertEngine {
    window_levels: HashMap<(String, String), AlertLevel>,
    balance_low: HashMap<String, bool>,
    /// Accounts whose baseline poll has happened. Keyed per account rather than
    /// held as one global flag, so one provider succeeding does not silently
    /// baseline another whose first fetch is still failing.
    baselined: HashSet<String>,
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
            let key = (snapshot.provider_id.clone(), w.label.clone());
            let prev = self
                .window_levels
                .insert(key, level)
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
}
