pub mod claude;
pub mod codex;
pub mod hermes;
pub mod openrouter;

use crate::config::Config;
use crate::model::{FetchError, UsageSnapshot};
use chrono::{DateTime, TimeZone, Utc};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// Everything an adapter needs to fetch: shared HTTP client, the user's home
/// directory (overridable in tests), pasted secrets, and the live config (for
/// per-provider settings like endpoint overrides).
pub struct ProviderCtx {
    pub http: reqwest::Client,
    pub home: PathBuf,
    pub secrets: HashMap<String, String>,
    pub config: Config,
}

impl ProviderCtx {
    pub fn new(home: PathBuf, secrets: HashMap<String, String>, config: Config) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("quota-widget/0.1")
            .build()
            .expect("reqwest client");
        Self { http, home, secrets, config }
    }
}

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &'static str;
    fn name(&self) -> &'static str;
    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError>;
}

/// The full adapter registry, in display order.
pub fn all_providers() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(claude::Claude),
        Box::new(codex::Codex),
        Box::new(openrouter::OpenRouter),
        Box::new(hermes::Hermes),
    ]
}

// ---- shared parsing helpers -------------------------------------------------

pub(crate) fn as_f64(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Parse a `resets_at`-style value: ISO-8601 string, epoch seconds, or epoch ms.
pub(crate) fn parse_timestamp(v: &serde_json::Value) -> Option<DateTime<Utc>> {
    if let Some(s) = v.as_str() {
        return DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&Utc));
    }
    let n = as_f64(v)?;
    let n = n as i64;
    if n > 100_000_000_000 {
        Utc.timestamp_millis_opt(n).single()
    } else {
        Utc.timestamp_opt(n, 0).single()
    }
}

pub(crate) fn network_err(e: reqwest::Error) -> FetchError {
    FetchError::Network(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_in_three_formats() {
        let iso = serde_json::json!("2026-08-01T10:00:00Z");
        let secs = serde_json::json!(1_785_542_400);
        let ms = serde_json::json!(1_785_542_400_000i64);
        assert!(parse_timestamp(&iso).is_some());
        assert_eq!(parse_timestamp(&secs), parse_timestamp(&ms));
    }
}
