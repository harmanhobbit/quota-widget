//! Shared plumbing for providers that answer "what's my balance?" with one
//! authenticated GET and a small JSON body.
//!
//! Every such adapter repeats the same six steps: read the pasted key, allow an
//! endpoint override, send a bearer-auth GET, map 401/403 to `AuthExpired` and
//! other non-2xx to `Network`, decode, and parse. Only the last step differs
//! between providers, so that is the only part each one writes: a `parse` fn
//! from the response body to `Credits`.
//!
//! Providers whose quantity is a per-cycle *allowance* rather than a balance do
//! not belong here — they emit a `UsageWindow` instead (see `elevenlabs.rs` and
//! `firecrawl.rs`).

use super::{network_err, Provider, ProviderCtx};
use crate::model::{Credits, FetchError, UsageSnapshot};
use serde_json::Value;

/// The per-provider facts that vary. Everything else is shared below.
pub struct CreditsSpec {
    pub kind: &'static str,
    pub display_name: &'static str,
    /// Endpoint used when the account has no override set.
    pub default_url: &'static str,
    /// Per-account setting name that overrides `default_url`. Needed where keys
    /// are platform-specific (Moonshot) or the host may move.
    pub url_setting: &'static str,
    /// Hint naming this provider's key, shown when nothing is pasted yet.
    pub not_configured: &'static str,
    /// Response body to `Credits`. `None` means the shape was unrecognisable,
    /// which surfaces as `FetchError::Parse`.
    pub parse: fn(&Value) -> Option<Credits>,
}

/// One configured account of a simple-credits provider.
pub struct SimpleCredits {
    pub key: String,
    pub label: Option<String>,
    pub spec: &'static CreditsSpec,
}

impl SimpleCredits {
    pub fn new(key: String, label: Option<String>, spec: &'static CreditsSpec) -> Self {
        Self { key, label, spec }
    }
}

#[async_trait::async_trait]
impl Provider for SimpleCredits {
    fn kind(&self) -> &'static str {
        self.spec.kind
    }
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        self.label.as_deref().unwrap_or(self.spec.display_name)
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let key = ctx
            .secrets
            .get(&self.key)
            .filter(|k| !k.is_empty())
            .ok_or_else(|| FetchError::NotConfigured(self.spec.not_configured.into()))?;
        let url = ctx
            .config
            .provider_setting(&self.key, self.spec.url_setting)
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| self.spec.default_url.to_string());

        let resp = ctx
            .http
            .get(&url)
            .bearer_auth(key)
            .send()
            .await
            .map_err(network_err)?;
        match resp.status().as_u16() {
            200..=299 => {}
            401 | 403 => {
                return Err(FetchError::AuthExpired(
                    "API key rejected — re-check it in Settings".into(),
                ))
            }
            s => return Err(FetchError::Network(format!("HTTP {s} from balance endpoint"))),
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        let credits = (self.spec.parse)(&body)
            .ok_or_else(|| FetchError::Parse("balance response missing totals".into()))?;
        Ok(UsageSnapshot::ok(
            self.id(),
            self.name(),
            vec![],
            Some(credits),
        ))
    }
}
