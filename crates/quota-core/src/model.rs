use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One rate-limit window of a provider (e.g. Claude's rolling 5-hour window).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UsageWindow {
    /// Stable machine-readable identity used by per-account UI selections.
    /// Empty in old serialized snapshots; callers then use automatic fallback.
    pub metric_id: String,
    pub label: String,
    /// 0.0–100.0. May exceed 100 if the provider reports overage.
    pub used_pct: f64,
    pub resets_at: Option<DateTime<Utc>>,
    /// When this window's current period began. Together with `resets_at` it
    /// lets the UI show how far through the period we are. `None` when the
    /// provider gives us no way to know.
    pub period_start: Option<DateTime<Utc>>,
    /// Shown, but never drives status, alerts, or the tray. For allowances
    /// whose exhaustion doesn't actually block you — e.g. Hermes' tiny free
    /// subscription trickle when a purchased balance is still funding calls.
    pub informational: bool,
}

impl Default for UsageWindow {
    fn default() -> Self {
        Self {
            metric_id: String::new(),
            label: String::new(),
            used_pct: 0.0,
            resets_at: None,
            period_start: None,
            informational: false,
        }
    }
}

/// Credit balance for pay-per-use providers (OpenRouter, Hermes Portal).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Credits {
    pub balance: f64,
    /// Display unit, e.g. "USD" or "credits".
    pub unit: String,
    pub used: Option<f64>,
    pub granted: Option<f64>,
    /// Estimated tokens remaining at the configured per-token price, if set.
    pub est_tokens_remaining: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, thiserror::Error)]
#[serde(tag = "kind", content = "detail")]
pub enum FetchError {
    /// Provider is enabled but has no usable credentials yet. Detail is a fix hint.
    #[error("not configured: {0}")]
    NotConfigured(String),
    /// Credentials exist but were rejected. Detail is a fix hint.
    #[error("auth expired: {0}")]
    AuthExpired(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("unexpected response: {0}")]
    Parse(String),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Warn,
    Critical,
    /// Fetch failed; last data (if any) is stale.
    Stale,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageSnapshot {
    pub provider_id: String,
    pub provider_name: String,
    pub windows: Vec<UsageWindow>,
    pub credits: Option<Credits>,
    pub fetched_at: DateTime<Utc>,
    pub error: Option<FetchError>,
}

impl UsageSnapshot {
    pub fn ok(id: &str, name: &str, windows: Vec<UsageWindow>, credits: Option<Credits>) -> Self {
        Self {
            provider_id: id.into(),
            provider_name: name.into(),
            windows,
            credits,
            fetched_at: Utc::now(),
            error: None,
        }
    }

    pub fn failed(id: &str, name: &str, err: FetchError) -> Self {
        Self {
            provider_id: id.into(),
            provider_name: name.into(),
            windows: vec![],
            credits: None,
            fetched_at: Utc::now(),
            error: Some(err),
        }
    }

    /// Worst status across this snapshot's windows/credits for the given thresholds.
    pub fn status(
        &self,
        warn_pct: f64,
        critical_pct: f64,
        low_balance_warn: Option<f64>,
    ) -> Status {
        if self.error.is_some() {
            return Status::Stale;
        }
        let mut worst = Status::Ok;
        for w in self.windows.iter().filter(|w| !w.informational) {
            let s = if w.used_pct >= critical_pct {
                Status::Critical
            } else if w.used_pct >= warn_pct {
                Status::Warn
            } else {
                Status::Ok
            };
            worst = worst.max(s);
        }
        if let (Some(c), Some(thr)) = (&self.credits, low_balance_warn) {
            if c.balance <= thr {
                worst = worst.max(Status::Warn);
            }
        }
        worst
    }
}
