pub mod anthropic_admin;
pub mod claude;
pub mod codex;
pub mod deepseek;
pub mod elevenlabs;
pub mod firecrawl;
pub mod fireworks;
pub mod grok;
pub mod hermes;
pub mod moonshot;
pub mod onehop;
pub mod openai_admin;
pub mod openrouter;
pub mod simple_credits;
pub mod spend;
pub mod venice;
pub mod zai;

use crate::config::Config;
use crate::model::{FetchError, UsageSnapshot};
use chrono::{DateTime, TimeZone, Utc};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One rotated credential an adapter asked the host to persist, and whether
/// that write succeeded. Collected by [`ProviderCtx::persist_secret`] and
/// drained by [`crate::refresh::refresh`] into its [`crate::refresh::RefreshOutcome`],
/// so a persistence failure is reported alongside the rest of one refresh
/// pass rather than only logged by the host in passing.
#[derive(Debug, Clone, PartialEq)]
pub struct CredentialWrite {
    pub key: String,
    pub value: String,
    pub result: Result<(), String>,
}

/// Everything an adapter needs to fetch: shared HTTP client, the user's home
/// directory (overridable in tests), pasted secrets, and the live config (for
/// per-provider settings like endpoint overrides).
pub struct ProviderCtx {
    pub http: reqwest::Client,
    pub home: PathBuf,
    /// The host-owned directory containing config.json and durable adapter
    /// state such as monthly-spend baselines.
    pub config_dir: PathBuf,
    pub spend_baselines: Arc<crate::spend_baseline::SpendBaselines>,
    pub secrets: HashMap<String, String>,
    pub config: Config,
    /// Called when an adapter rotates a stored credential (e.g. an OAuth
    /// refresh) so the host can persist it. Key is the secret name. Returns
    /// whether the write succeeded, which `persist_secret` records.
    pub on_secret_update: Option<SecretUpdateHook>,
    /// Every credential write requested during this context's lifetime (one
    /// refresh pass), in call order.
    credential_writes: Mutex<Vec<CredentialWrite>>,
}

/// Host callback invoked as `(secret_name, value)` when an adapter rotates a
/// stored credential. Returns `Err` with a reason when the write failed.
pub type SecretUpdateHook = std::sync::Arc<dyn Fn(&str, &str) -> Result<(), String> + Send + Sync>;

impl ProviderCtx {
    pub fn new(
        home: PathBuf,
        config_dir: PathBuf,
        secrets: HashMap<String, String>,
        config: Config,
    ) -> Self {
        // ADR 0006 (Android) says HTTPS is "validated through the Android
        // system trust store". This client uses reqwest's default `rustls-tls`
        // (bundled webpki-roots, workspace Cargo.toml) unconditionally on
        // every platform, Android included, instead of wiring in Android's
        // real system certificate verifier — that needs a Kotlin component
        // bundled into the generated Gradle project
        // (rustls-platform-verifier's own documented requirement), which is a
        // meaningfully larger, hard-to-verify-headless surface than issue
        // #109 scoped in. webpki-roots is a fixed public-CA bundle: it already
        // satisfies the criterion's actual testable behavior — cleartext HTTP,
        // a private/self-signed CA, and any bypass are all rejected — just not
        // via the literal system store. Flagged here per docs/agents/domain.md
        // ("flag ADR conflicts") rather than silently narrowed; literal
        // system-trust-store verification is a candidate for a follow-up
        // ticket if a real need for it (e.g. an enterprise MITM proxy Android
        // trusts) shows up.
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .user_agent("quota-widget/0.1")
            .build()
            .expect("reqwest client");
        Self {
            http,
            home,
            spend_baselines: Arc::new(crate::spend_baseline::SpendBaselines::new(
                config_dir.clone(),
            )),
            config_dir,
            secrets,
            config,
            on_secret_update: None,
            credential_writes: Mutex::new(Vec::new()),
        }
    }

    pub fn persist_secret(&self, key: &str, value: &str) {
        let result = match &self.on_secret_update {
            Some(f) => f(key, value),
            None => Ok(()),
        };
        self.credential_writes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(CredentialWrite {
                key: key.to_string(),
                value: value.to_string(),
                result,
            });
    }

    /// Every credential write requested so far, in call order. Non-consuming:
    /// callers ([`crate::refresh::refresh`], tests) can inspect without ending
    /// the context's lifetime.
    pub fn credential_writes(&self) -> Vec<CredentialWrite> {
        self.credential_writes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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
        ("grok", "Grok"),
        ("openrouter", "OpenRouter"),
        ("elevenlabs", "ElevenLabs"),
        ("firecrawl", "Firecrawl"),
        ("deepseek", "DeepSeek"),
        ("moonshot", "Moonshot"),
        ("venice", "Venice"),
        ("onehop", "OneHop"),
        ("fireworks", "Fireworks"),
        ("anthropic_admin", "Anthropic Admin"),
        ("openai_admin", "OpenAI Admin"),
        ("hermes", "Nous"),
        ("zai", "Z.ai"),
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
                "grok" => Some(Box::new(grok::Grok::new(key.clone(), label)) as Box<dyn Provider>),
                "openrouter" => {
                    Some(Box::new(openrouter::OpenRouter::new(key.clone(), label))
                        as Box<dyn Provider>)
                }
                "elevenlabs" => {
                    Some(Box::new(elevenlabs::ElevenLabs::new(key.clone(), label))
                        as Box<dyn Provider>)
                }
                "firecrawl" => {
                    Some(Box::new(firecrawl::Firecrawl::new(key.clone(), label))
                        as Box<dyn Provider>)
                }
                "deepseek" => Some(Box::new(simple_credits::SimpleCredits::new(
                    key.clone(),
                    label,
                    &deepseek::SPEC,
                )) as Box<dyn Provider>),
                "moonshot" => Some(Box::new(simple_credits::SimpleCredits::new(
                    key.clone(),
                    label,
                    &moonshot::SPEC,
                )) as Box<dyn Provider>),
                "venice" => {
                    Some(Box::new(venice::Venice::new(key.clone(), label)) as Box<dyn Provider>)
                }
                "onehop" => Some(Box::new(simple_credits::SimpleCredits::new(
                    key.clone(),
                    label,
                    &onehop::SPEC,
                )) as Box<dyn Provider>),
                "fireworks" => {
                    Some(Box::new(fireworks::Fireworks::new(key.clone(), label))
                        as Box<dyn Provider>)
                }
                "anthropic_admin" => Some(Box::new(anthropic_admin::AnthropicAdmin::new(
                    key.clone(),
                    label,
                )) as Box<dyn Provider>),
                "openai_admin" => {
                    Some(Box::new(openai_admin::OpenAiAdmin::new(key.clone(), label))
                        as Box<dyn Provider>)
                }
                "hermes" => {
                    Some(Box::new(hermes::Hermes::new(key.clone(), label)) as Box<dyn Provider>)
                }
                "zai" => Some(Box::new(zai::Zai::new(key.clone(), label)) as Box<dyn Provider>),
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

/// `reqwest::Error`'s Display is only the top layer — always "error sending
/// request for url (…)" for send/connect failures — while the cause that
/// actually distinguishes a DNS failure from a refused or reset connection
/// lives in the source chain. Keep the chain: without it a transient failure
/// is undiagnosable from the error the UI shows.
pub(crate) fn network_err(e: reqwest::Error) -> FetchError {
    let mut msg = e.to_string();
    let mut src = std::error::Error::source(&e);
    while let Some(s) = src {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        src = s.source();
    }
    FetchError::Network(msg)
}

/// Guards the one user-editable endpoint among the direct-HTTPS providers
/// (Moonshot's `balance_url`) against a pasted `http://` URL or something
/// that isn't a URL at all. Every other adapter's endpoint is a hardcoded
/// `https://` constant, so this is the single place a non-HTTPS request could
/// otherwise reach the network — required on every platform, not just
/// Android, since the setting round-trips through the same `Config` on
/// desktop.
pub(crate) fn require_https(url: &str) -> Result<(), FetchError> {
    match reqwest::Url::parse(url) {
        Ok(u) if u.scheme() == "https" => Ok(()),
        Ok(_) => Err(FetchError::NotConfigured(format!(
            "endpoint must be https://, not {url}"
        ))),
        Err(_) => Err(FetchError::NotConfigured(format!(
            "endpoint is not a valid URL: {url}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn network_err_keeps_the_cause_chain() {
        // A guaranteed-closed port: bound, then released, so connecting
        // refuses rather than hanging.
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let err = reqwest::Client::new()
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .unwrap_err();
        let top = err.to_string();
        let FetchError::Network(msg) = network_err(err) else {
            panic!("expected FetchError::Network");
        };
        assert!(msg.starts_with(&top), "{msg} should keep the top layer");
        // The layer that says *why* — "client error (Connect)" / "Connection
        // refused" — must survive into the message.
        assert!(msg.len() > top.len(), "{msg} should append the causes");
        assert!(msg.contains("Connect"), "{msg}");
    }

    #[test]
    fn require_https_accepts_only_https_urls() {
        assert!(require_https("https://api.moonshot.ai/v1/users/me/balance").is_ok());
        assert!(matches!(
            require_https("http://api.moonshot.ai/v1/users/me/balance"),
            Err(FetchError::NotConfigured(_))
        ));
        assert!(matches!(
            require_https("not a url"),
            Err(FetchError::NotConfigured(_))
        ));
    }

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

    /// A configured `zai` account instantiates as the built-in adapter, keeps
    /// its immutable account key as its id (the secret name derives from it)
    /// and presents a custom label where one is set (issue #183).
    #[test]
    fn configured_zai_account_is_instantiated_with_key_and_label() {
        let mut cfg = Config::default();
        cfg.providers.insert(
            "zai#personal".into(),
            crate::config::ProviderConfig {
                enabled: true,
                kind: Some("zai".into()),
                label: Some("Work Z.ai".into()),
                ..Default::default()
            },
        );
        let providers = providers_for(&cfg);
        let account = providers
            .iter()
            .find(|p| p.id() == "zai#personal")
            .expect("a configured zai kind must instantiate");
        assert_eq!(account.kind(), "zai");
        assert_eq!(account.id(), "zai#personal", "account key is the identity");
        assert_eq!(account.name(), "Work Z.ai", "custom label wins");
        // No label set: the provider's display name, not the model names.
        cfg.providers.insert("zai".into(), Default::default());
        let providers = providers_for(&cfg);
        let plain = providers.iter().find(|p| p.id() == "zai").unwrap();
        assert_eq!(plain.kind(), "zai");
        assert_eq!(plain.name(), "Z.ai");
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
            vec![
                "grok",
                "openrouter",
                "elevenlabs",
                "firecrawl",
                "deepseek",
                "moonshot",
                "venice",
                "onehop",
                "fireworks",
                "anthropic_admin",
                "openai_admin",
                "hermes",
                "zai",
                "codex",
                "claude"
            ]
        );
    }
}
