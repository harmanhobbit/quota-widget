//! Persisted month-start readings for providers which only expose a lifetime
//! total (OpenRouter) or a remaining balance (Hermes fallback responses).
//!
//! This deliberately lives in quota-core: adapters own the interpretation of
//! their readings, while the host supplies the config directory through
//! `ProviderCtx`. A broken or absent file is treated exactly like a first poll,
//! never as spend from an unknown earlier month.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

const FILE_NAME: &str = "spend-baselines.json";

#[derive(Default, Serialize, Deserialize)]
struct BaselineFile {
    #[serde(default)]
    entries: HashMap<String, Baseline>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Baseline {
    month: String,
    value: f64,
    #[serde(default)]
    accrued: f64,
}

/// One in-memory view per poll cycle makes concurrent adapter fetches update
/// one file coherently. The file is still reloaded for each new context, so it
/// survives restarts without quota-core having to know the app's paths.
pub struct SpendBaselines {
    path: PathBuf,
    entries: Mutex<BaselineFile>,
}

impl SpendBaselines {
    pub fn new(config_dir: PathBuf) -> Self {
        let path = config_dir.join(FILE_NAME);
        let entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            path,
            entries: Mutex::new(entries),
        }
    }

    /// Convert a provider's lifetime cumulative spend into this-month spend.
    /// A total going backwards is an account reset (or a changed API), so a
    /// fresh baseline is safer than showing a negative amount.
    pub fn cumulative_spend(&self, key: &str, current: f64, now: DateTime<Utc>) -> f64 {
        let month = month_key(now);
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries.entries.entry(key.to_owned()).or_insert(Baseline {
            month: month.clone(),
            value: current,
            accrued: 0.0,
        });
        let spend = if entry.month != month || current < entry.value {
            *entry = Baseline {
                month,
                value: current,
                accrued: 0.0,
            };
            0.0
        } else {
            current - entry.value
        };
        self.save(&entries);
        spend
    }

    /// Convert a remaining balance into this-month spend. A top-up increases
    /// the balance; retain spend accrued before it and start measuring future
    /// drawdown from the higher balance rather than inventing negative spend.
    pub fn balance_spend(&self, key: &str, current: f64, now: DateTime<Utc>) -> f64 {
        let month = month_key(now);
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = entries.entries.entry(key.to_owned()).or_insert(Baseline {
            month: month.clone(),
            value: current,
            accrued: 0.0,
        });
        if entry.month != month {
            *entry = Baseline {
                month,
                value: current,
                accrued: 0.0,
            };
        } else if current <= entry.value {
            entry.accrued += entry.value - current;
            entry.value = current;
        } else {
            // A balance rise is a top-up. Keep the spending accumulated so far.
            entry.value = current;
        }
        let spend = entry.accrued;
        self.save(&entries);
        spend
    }

    fn save(&self, entries: &BaselineFile) {
        let Some(parent) = self.path.parent() else {
            return;
        };
        let Ok(text) = serde_json::to_string(entries) else {
            return;
        };
        if std::fs::create_dir_all(parent).is_ok() {
            let tmp = self.path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                let _ = std::fs::rename(tmp, &self.path);
            }
        }
    }
}

fn month_key(now: DateTime<Utc>) -> String {
    // `spend::month_bounds` is the calendar authority used by every monthly
    // target. Its start instant is a stable, unambiguous persistence key.
    crate::providers::spend::month_bounds(now)
        .expect("UTC calendar month has bounds")
        .0
        .to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn at(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    #[test]
    fn cumulative_baselines_at_month_start_and_resets_on_rollover() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = SpendBaselines::new(dir.path().into());
            assert_eq!(
                store.cumulative_spend("openrouter", 25.0, at("2026-08-04T12:00:00Z")),
                0.0
            );
        }
        // A fresh context represents the next app poll (or a restart).
        let store = SpendBaselines::new(dir.path().into());
        assert_eq!(
            store.cumulative_spend("openrouter", 28.5, at("2026-08-20T12:00:00Z")),
            3.5
        );
        assert_eq!(
            store.cumulative_spend("openrouter", 31.0, at("2026-09-01T00:00:00Z")),
            0.0
        );
    }

    #[test]
    fn lower_cumulative_reading_rebaselines_instead_of_going_negative() {
        let dir = tempfile::tempdir().unwrap();
        let store = SpendBaselines::new(dir.path().into());
        assert_eq!(
            store.cumulative_spend("openrouter", 25.0, at("2026-08-04T12:00:00Z")),
            0.0
        );
        assert_eq!(
            store.cumulative_spend("openrouter", 5.0, at("2026-08-05T12:00:00Z")),
            0.0
        );
        assert_eq!(
            store.cumulative_spend("openrouter", 6.0, at("2026-08-06T12:00:00Z")),
            1.0
        );
    }

    #[test]
    fn corrupt_file_is_a_new_baseline_not_bad_spend() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), "not json").unwrap();
        let store = SpendBaselines::new(dir.path().into());
        assert_eq!(
            store.cumulative_spend("openrouter", 25.0, at("2026-08-04T12:00:00Z")),
            0.0
        );
    }

    #[test]
    fn balance_top_up_keeps_already_accrued_spend() {
        let dir = tempfile::tempdir().unwrap();
        let store = SpendBaselines::new(dir.path().into());
        assert_eq!(
            store.balance_spend("hermes", 100.0, at("2026-08-04T12:00:00Z")),
            0.0
        );
        assert_eq!(
            store.balance_spend("hermes", 90.0, at("2026-08-05T12:00:00Z")),
            10.0
        );
        assert_eq!(
            store.balance_spend("hermes", 140.0, at("2026-08-06T12:00:00Z")),
            10.0
        );
        assert_eq!(
            store.balance_spend("hermes", 130.0, at("2026-08-07T12:00:00Z")),
            20.0
        );
    }
}
