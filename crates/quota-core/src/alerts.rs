use crate::config::{Config, Thresholds};
use crate::model::UsageSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertLevel {
    Normal,
    Warn,
    Critical,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct AlertEvent {
    pub provider_id: String,
    pub provider_name: String,
    pub level: AlertLevel,
    /// e.g. "5-hour window at 87%" or "balance low: 1.42 USD"
    pub message: String,
}

/// Edge-triggered alert engine: an event fires only when a metric *crosses up*
/// into warn/critical territory, and re-arms once it drops back below (i.e.
/// after the window resets). Polling at 85% every minute produces one alert,
/// not sixty.
#[derive(Default)]
pub struct AlertEngine {
    window_levels: HashMap<(String, String), AlertLevel>,
    balance_low: HashMap<String, bool>,
}

impl AlertEngine {
    pub fn evaluate(&mut self, snapshot: &UsageSnapshot, cfg: &Config) -> Vec<AlertEvent> {
        let mut events = Vec::new();
        if snapshot.error.is_some() {
            return events; // never alert off stale/failed data
        }
        let Thresholds { warn_pct, critical_pct } = cfg.effective_thresholds(&snapshot.provider_id);

        for w in snapshot.windows.iter().filter(|w| !w.informational) {
            let level = if w.used_pct >= critical_pct {
                AlertLevel::Critical
            } else if w.used_pct >= warn_pct {
                AlertLevel::Warn
            } else {
                AlertLevel::Normal
            };
            let key = (snapshot.provider_id.clone(), w.label.clone());
            let prev = self.window_levels.insert(key, level).unwrap_or(AlertLevel::Normal);
            if level > prev {
                events.push(AlertEvent {
                    provider_id: snapshot.provider_id.clone(),
                    provider_name: snapshot.provider_name.clone(),
                    level,
                    message: format!("{} window at {:.0}%", w.label, w.used_pct),
                });
            }
        }

        if let Some(credits) = &snapshot.credits {
            if let Some(thr) = cfg.providers.get(&snapshot.provider_id).and_then(|p| p.low_balance_warn) {
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
            vec![UsageWindow { label: "5-hour".into(), used_pct: pct, ..Default::default() }],
            None,
        )
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
        let ev = eng.evaluate(&snap(99.0), &cfg);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].level, AlertLevel::Critical);
    }

    #[test]
    fn no_alerts_from_errored_snapshots() {
        let cfg = Config::default();
        let mut eng = AlertEngine::default();
        let s = UsageSnapshot::failed("claude", "Claude", crate::model::FetchError::Network("x".into()));
        assert!(eng.evaluate(&s, &cfg).is_empty());
    }

    #[test]
    fn low_balance_fires_once() {
        let mut cfg = Config::default();
        cfg.providers.get_mut("openrouter").unwrap().low_balance_warn = Some(5.0);
        let mut eng = AlertEngine::default();
        let s = UsageSnapshot::ok(
            "openrouter",
            "OpenRouter",
            vec![],
            Some(Credits { balance: 3.0, unit: "USD".into(), used: None, granted: None, est_tokens_remaining: None }),
        );
        assert_eq!(eng.evaluate(&s, &cfg).len(), 1);
        assert!(eng.evaluate(&s, &cfg).is_empty());
    }
}
