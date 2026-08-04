pub mod claude;
pub mod codex;
pub mod elevenlabs;
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
    /// Called when an adapter rotates a stored credential (e.g. an OAuth
    /// refresh) so the host can persist it. Key is the secret name.
    pub on_secret_update: Option<std::sync::Arc<dyn Fn(&str, &str) + Send + Sync>>,
}

impl ProviderCtx {
    pub fn new(home: PathBuf, secrets: HashMap<String, String>, config: Config) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("quota-widget/0.1")
            .build()
            .expect("reqwest client");
        Self {
            http,
            home,
            secrets,
            config,
            on_secret_update: None,
        }
    }

    pub fn persist_secret(&self, key: &str, value: &str) {
        if let Some(f) = &self.on_secret_update {
            f(key, value);
        }
    }
}

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn kind(&self) -> &'static str;
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError>;
}

/// The full adapter registry, in display order.
pub fn adapter_kinds() -> &'static [(&'static str, &'static str)] {
    &[
        ("claude", "Claude"),
        ("codex", "Codex"),
        ("openrouter", "OpenRouter"),
        ("elevenlabs", "ElevenLabs"),
        ("hermes", "Hermes Portal"),
    ]
}

/// Instantiate each configured account in stable map order. Unknown kinds are
/// ignored so a config written by a newer build remains usable.
pub fn providers_for(cfg: &Config) -> Vec<Box<dyn Provider>> {
    cfg.providers
        .iter()
        .filter_map(|(key, entry)| {
            let kind = entry.kind.as_deref().unwrap_or(key);
            let label = entry.label.clone();
            match kind {
                "claude" => {
                    Some(Box::new(claude::Claude::new(key.clone(), label)) as Box<dyn Provider>)
                }
                "codex" => {
                    Some(Box::new(codex::Codex::new(key.clone(), label)) as Box<dyn Provider>)
                }
                "openrouter" => {
                    Some(Box::new(openrouter::OpenRouter::new(key.clone(), label))
                        as Box<dyn Provider>)
                }
                "elevenlabs" => {
                    Some(Box::new(elevenlabs::ElevenLabs::new(key.clone(), label))
                        as Box<dyn Provider>)
                }
                "hermes" => {
                    Some(Box::new(hermes::Hermes::new(key.clone(), label)) as Box<dyn Provider>)
                }
                _ => None,
            }
        })
        .collect()
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
        return DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc));
    }
    let n = as_f64(v)?;
    let n = n as i64;
    if n > 100_000_000_000 {
        Utc.timestamp_millis_opt(n).single()
    } else {
        Utc.timestamp_opt(n, 0).single()
    }
}

/// Start of the billing cycle ending at `resets_at`, for providers whose period
/// is a calendar month. Calendar arithmetic, not a fixed 30 days: a cycle
/// ending 3 March began 3 February, and the progress marker would otherwise
/// drift by up to three days depending on the month.
///
/// Days past the 28th have no counterpart in every month; chrono's
/// `checked_sub_months` clamps them to the previous month's last day, which is
/// the same rule billing cycles themselves use.
pub(crate) fn calendar_month_start(resets_at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    resets_at.checked_sub_months(chrono::Months::new(1))
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

    #[test]
    fn configured_accounts_keep_keys_and_use_labels() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "claude#work".into(),
            crate::config::ProviderConfig {
                enabled: true,
                kind: Some("claude".into()),
                label: Some("Work Claude".into()),
                ..Default::default()
            },
        );
        let providers = providers_for(&cfg);
        assert_eq!(providers.iter().filter(|p| p.kind() == "claude").count(), 2);
        let work = providers.iter().find(|p| p.id() == "claude#work").unwrap();
        assert_eq!(work.name(), "Work Claude");
        assert_eq!(
            providers
                .iter()
                .find(|p| p.id() == "claude")
                .unwrap()
                .name(),
            "Claude"
        );
    }

    #[test]
    fn configured_account_order_is_provider_display_order() {
        let mut cfg = Config::default();
        let codex = cfg.providers.shift_remove("codex").unwrap();
        let claude = cfg.providers.shift_remove("claude").unwrap();
        cfg.providers.insert("codex".into(), codex);
        cfg.providers.insert("claude".into(), claude);

        assert_eq!(
            providers_for(&cfg)
                .iter()
                .map(|provider| provider.id())
                .collect::<Vec<_>>(),
            vec!["openrouter", "elevenlabs", "hermes", "codex", "claude"]
        );
    }
}
