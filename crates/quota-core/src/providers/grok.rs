//! Grok (SuperGrok subscription) usage via the same `proxy.grok.com` billing
//! endpoint xAI's open-sourced `grok` CLI calls for its `/usage` command. These
//! are unofficial, CLI-internal endpoints and may change.
//!
//! Auth sources, controlled by the `auth_mode` provider setting (mirrors
//! `claude.rs`):
//! - `"cli"`   — only the grok CLI's `~/.grok/auth.json`
//! - `"oauth"` — only the widget's own device sign-in (the `grok_oauth` secret)
//! - `"auto"` (default) — a fresh CLI token wins; otherwise the widget's own
//!   stored login (refreshed as needed); otherwise a last-resort refresh with
//!   the CLI's refresh token.
//!
//! xAI rotates refresh tokens on use, so whenever *we* perform a refresh the
//! rotated pair is persisted to our own `grok_oauth` secret — never written
//! back into the CLI's `auth.json`. That keeps the CLI's login untouched and
//! means we refresh at most once per expiry, not once per poll.
//!
//! Constants (issuer, client id, proxy base, headers) are pinned to
//! `github.com/xai-org/grok-build` as of the clone this was written against.

use super::{network_err, Provider, ProviderCtx};
use crate::model::{Credits, FetchError, UsageSnapshot, UsageWindow};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

pub struct Grok {
    pub key: String,
    pub label: Option<String>,
}

/// Tokens stored by the widget's own device-code sign-in.
pub const OAUTH_SECRET_KEY: &str = "grok_oauth";

/// SuperGrok credits/usage endpoint the CLI's `/usage` hits (via the chat proxy).
const BILLING_URL: &str = "https://proxy.grok.com/v1/billing?format=credits";
/// xAI OAuth2 issuer; token refresh lives at `{issuer}/oauth2/token`.
const TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";
/// The grok CLI's public OAuth client id.
pub const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
/// Value of the `X-XAI-Token-Auth` header the CLI sends on billing requests.
const TOKEN_AUTH_HEADER: &str = "xai-grok-cli";
/// `x-grok-client-version` the proxy uses to segment/gate clients. Pinned to
/// the grok-build clone; a plausible current CLI version avoids a min-version
/// gate rejecting an otherwise-valid token.
const CLIENT_VERSION: &str = "1.0.4";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Auto,
    Cli,
    Oauth,
}

pub fn auth_mode(ctx: &ProviderCtx, key: &str) -> AuthMode {
    match ctx
        .config
        .provider_setting(key, "auth_mode")
        .and_then(|v| v.as_str().map(str::to_owned))
        .as_deref()
    {
        Some("cli") => AuthMode::Cli,
        Some("oauth") => AuthMode::Oauth,
        _ => AuthMode::Auto,
    }
}

/// A usable token set from either source. Unlike Claude's, this carries the
/// `user_id` the billing endpoint needs in its `x-userid` header.
#[derive(Debug, Clone, PartialEq)]
pub struct GrokTokens {
    pub access: String,
    pub refresh: Option<String>,
    /// epoch millis; 0 = unknown (treat as valid, let the API reject it)
    pub expires_at_ms: i64,
    pub user_id: Option<String>,
}

impl GrokTokens {
    pub fn expired(&self, now_ms: i64) -> bool {
        self.expires_at_ms > 0 && self.expires_at_ms < now_ms + 60_000
    }

    pub fn to_secret_json(&self) -> String {
        serde_json::json!({
            "accessToken": self.access,
            "refreshToken": self.refresh,
            "expiresAt": self.expires_at_ms,
            "userId": self.user_id,
        })
        .to_string()
    }
}

/// Parse the widget's own stored secret (accessToken/refreshToken/expiresAt/userId).
pub fn parse_grok_tokens(v: &Value) -> Option<GrokTokens> {
    let access = v["accessToken"].as_str()?.to_string();
    if access.is_empty() {
        return None;
    }
    Some(GrokTokens {
        access,
        refresh: v["refreshToken"].as_str().map(String::from),
        expires_at_ms: v["expiresAt"].as_i64().unwrap_or(0),
        user_id: v["userId"].as_str().map(String::from),
    })
}

#[async_trait::async_trait]
impl Provider for Grok {
    fn kind(&self) -> &'static str {
        "grok"
    }
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        self.label.as_deref().unwrap_or("Grok")
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let auth = self.resolve(ctx).await?;
        let url = ctx
            .config
            .provider_setting(&self.key, "usage_url")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| BILLING_URL.to_string());

        let mut req = ctx
            .http
            .get(&url)
            .bearer_auth(&auth.access)
            .header("X-XAI-Token-Auth", TOKEN_AUTH_HEADER)
            .header("x-grok-client-version", CLIENT_VERSION)
            .header("Accept", "application/json");
        if let Some(uid) = &auth.user_id {
            req = req.header("x-userid", uid);
        }
        let resp = req.send().await.map_err(network_err)?;
        match resp.status().as_u16() {
            200..=299 => {}
            401 | 403 => return Err(FetchError::AuthExpired(reauth_hint(auth_mode(ctx, &self.key)))),
            s => return Err(FetchError::Network(format!("HTTP {s} from billing endpoint"))),
        }
        let body: BillingResponse = resp.json().await.map_err(network_err)?;
        let config = body
            .config
            .ok_or_else(|| FetchError::Parse("billing response had no config".into()))?;
        let (windows, credits) = parse_billing(&config);
        if windows.is_empty() && credits.is_none() {
            return Err(FetchError::Parse(
                "billing response had neither a usage allowance nor a credit balance".into(),
            ));
        }
        Ok(UsageSnapshot::ok(self.id(), self.name(), windows, credits))
    }
}

fn reauth_hint(mode: AuthMode) -> String {
    match mode {
        AuthMode::Cli => "token rejected — run `grok` once to refresh the login".into(),
        AuthMode::Oauth => "token rejected — sign in again in Settings → Grok".into(),
        AuthMode::Auto => {
            "token rejected — sign in via Settings → Grok (or run `grok` if you use the CLI)".into()
        }
    }
}

fn notconf_hint(mode: AuthMode) -> String {
    match mode {
        AuthMode::Cli => "no grok CLI login found — install the grok CLI and run `grok`".into(),
        AuthMode::Oauth => "not signed in — use Settings → Grok → Sign in".into(),
        AuthMode::Auto => {
            "no Grok login found — sign in via Settings → Grok (or run `grok` if you use the CLI)"
                .into()
        }
    }
}

impl Grok {
    pub fn new(key: String, label: Option<String>) -> Self {
        Self { key, label }
    }
    fn secret_key(&self) -> String {
        format!("{}_oauth", self.key)
    }

    /// Best OIDC/OAuth2 session credential from the grok CLI's `~/.grok/auth.json`.
    /// The file is a map of scope → auth entry; the plain `xai::api_key` scope and
    /// legacy web-login entries are skipped (they can't fetch consumer billing),
    /// and the most recently minted session is preferred.
    fn cli_tokens(&self, ctx: &ProviderCtx) -> Option<GrokTokens> {
        let path = ctx.home.join(".grok").join("auth.json");
        let text = std::fs::read_to_string(&path).ok()?;
        let store: Value = serde_json::from_str(&text).ok()?;
        let entries = store.as_object()?;

        let mut best: Option<(DateTime<Utc>, GrokTokens)> = None;
        for (scope, entry) in entries {
            if scope == "xai::api_key" {
                continue;
            }
            // Session tokens only; a bare API key or legacy web login can't read
            // the consumer billing endpoint.
            match entry["auth_mode"].as_str() {
                Some("api_key") | Some("web_login") | Some("grok") => continue,
                _ => {}
            }
            let access = match entry["key"].as_str() {
                Some(k) if !k.is_empty() => k.to_string(),
                _ => continue,
            };
            let tokens = GrokTokens {
                access,
                refresh: entry["refresh_token"].as_str().map(String::from),
                expires_at_ms: entry["expires_at"]
                    .as_str()
                    .and_then(parse_rfc3339)
                    .map(|d| d.timestamp_millis())
                    .unwrap_or(0),
                user_id: entry["user_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(String::from),
            };
            // `create_time` orders candidates; missing/unparseable sorts oldest.
            let minted = entry["create_time"]
                .as_str()
                .and_then(parse_rfc3339)
                .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).unwrap());
            if best.as_ref().is_none_or(|(t, _)| minted >= *t) {
                best = Some((minted, tokens));
            }
        }
        best.map(|(_, t)| t)
    }

    fn stored_tokens(&self, ctx: &ProviderCtx) -> Option<GrokTokens> {
        let raw = ctx.secrets.get(&self.secret_key())?;
        parse_grok_tokens(&serde_json::from_str(raw).ok()?)
    }

    async fn resolve(&self, ctx: &ProviderCtx) -> Result<GrokTokens, FetchError> {
        let mode = auth_mode(ctx, &self.key);
        let now_ms = Utc::now().timestamp_millis();

        // 1. A fresh token from the grok CLI file (kept current by the CLI
        //    itself whenever the user runs it).
        let cli = if mode != AuthMode::Oauth {
            self.cli_tokens(ctx)
        } else {
            None
        };
        if let Some(t) = &cli {
            if !t.expired(now_ms) {
                return Ok(t.clone());
            }
        }

        // 2. Our own stored login, refreshed (and re-persisted) as needed.
        if mode != AuthMode::Cli {
            if let Some(t) = self.stored_tokens(ctx) {
                if !t.expired(now_ms) {
                    return Ok(t);
                }
                if t.refresh.is_some() {
                    if let Ok(fresh) = self.refresh(ctx, &t).await {
                        ctx.persist_secret(&self.secret_key(), &fresh.to_secret_json());
                        return Ok(fresh);
                    }
                }
                return Err(FetchError::AuthExpired(reauth_hint(mode)));
            }
        }

        // 3. Last resort: refresh with the CLI's refresh token. The rotated pair
        //    is stored under our secret, not written back to the CLI file.
        if let Some(t) = cli {
            if t.refresh.is_some() {
                if let Ok(fresh) = self.refresh(ctx, &t).await {
                    ctx.persist_secret(&self.secret_key(), &fresh.to_secret_json());
                    return Ok(fresh);
                }
            }
            return Err(FetchError::AuthExpired(reauth_hint(mode)));
        }

        Err(FetchError::NotConfigured(notconf_hint(mode)))
    }

    /// Exchange a refresh token at `{issuer}/oauth2/token` (form-encoded, per the
    /// grok CLI). `user_id` isn't returned by the refresh grant, so it's carried
    /// forward from `old`.
    async fn refresh(&self, ctx: &ProviderCtx, old: &GrokTokens) -> Result<GrokTokens, FetchError> {
        let refresh_token = old
            .refresh
            .as_deref()
            .ok_or_else(|| FetchError::AuthExpired("no refresh token".into()))?;
        let resp = ctx
            .http
            .post(TOKEN_URL)
            .header("x-grok-client-version", CLIENT_VERSION)
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh_token),
                ("client_id", CLIENT_ID),
            ])
            .send()
            .await
            .map_err(network_err)?;
        if !resp.status().is_success() {
            return Err(FetchError::AuthExpired(format!(
                "refresh failed: HTTP {}",
                resp.status()
            )));
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        let access = body["access_token"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| FetchError::Parse("refresh response missing access_token".into()))?;
        let expires_at_ms = body["expires_in"]
            .as_i64()
            .map(|s| Utc::now().timestamp_millis() + s * 1000)
            .unwrap_or(0);
        Ok(GrokTokens {
            access,
            // xAI rotates refresh tokens; fall back to the one we used if the
            // response doesn't include a new one.
            refresh: body["refresh_token"]
                .as_str()
                .map(String::from)
                .or_else(|| Some(refresh_token.into())),
            expires_at_ms,
            user_id: old.user_id.clone(),
        })
    }
}

fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

// ---- billing response --------------------------------------------------------

#[derive(Debug, Deserialize)]
struct BillingResponse {
    config: Option<BillingConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BillingConfig {
    /// Included allowance used, 0–100. Preferred over deriving from used/limit.
    credit_usage_percent: Option<f64>,
    /// Current weekly/monthly period; `end` is the reset.
    current_period: Option<UsagePeriod>,
    /// Deprecated (legacy shape); fall back to these for the percentage.
    monthly_limit: Option<Cent>,
    used: Option<Cent>,
    /// Remaining prepaid (bought) credit balance, USD cents.
    prepaid_balance: Option<Cent>,
    /// Deprecated (legacy shape) period markers.
    billing_period_start: Option<String>,
    billing_period_end: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsagePeriod {
    #[serde(rename = "type")]
    period_type: Option<String>,
    start: Option<String>,
    end: Option<String>,
}

/// USD cents. proto3 JSON omits zero-valued scalars, so a `$0` Cent arrives as
/// `{}`; default to 0 rather than failing the parse.
#[derive(Debug, Deserialize)]
struct Cent {
    #[serde(default)]
    val: i64,
}

/// Build the allowance window (when a usage percentage is derivable) and the
/// prepaid-credits line (when the account has topped up) from a billing config.
fn parse_billing(config: &BillingConfig) -> (Vec<UsageWindow>, Option<Credits>) {
    let pct = config.credit_usage_percent.or_else(|| {
        match (config.monthly_limit.as_ref(), config.used.as_ref()) {
            (Some(limit), Some(used)) if limit.val > 0 => {
                Some(used.val as f64 / limit.val as f64 * 100.0)
            }
            _ => None,
        }
    });

    let period = config.current_period.as_ref();
    let resets_at = period
        .and_then(|p| p.end.as_deref())
        .or(config.billing_period_end.as_deref())
        .and_then(parse_rfc3339);
    let period_start = period
        .and_then(|p| p.start.as_deref())
        .or(config.billing_period_start.as_deref())
        .and_then(parse_rfc3339);

    let label = match period.and_then(|p| p.period_type.as_deref()) {
        Some(t) if t.contains("WEEKLY") => "Weekly allowance",
        Some(t) if t.contains("MONTHLY") => "Monthly allowance",
        _ => "Allowance",
    };

    let mut windows = Vec::new();
    if let Some(pct) = pct {
        windows.push(UsageWindow {
            metric_id: "allowance".into(),
            label: label.into(),
            used_pct: pct,
            resets_at,
            period_start,
            ..Default::default()
        });
    }

    // Only show a prepaid line when there's actually a positive balance, so
    // accounts that never top up don't display a $0.00 credits line.
    let credits = config.prepaid_balance.as_ref().filter(|c| c.val > 0).map(|c| Credits {
        balance: c.val as f64 / 100.0,
        label: Some("Prepaid credits".into()),
        unit: "USD".into(),
        used: None,
        granted: None,
        est_tokens_remaining: None,
    });

    (windows, credits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::collections::HashMap;

    fn config_of(json: serde_json::Value) -> BillingConfig {
        let resp: BillingResponse = serde_json::from_value(json).unwrap();
        resp.config.unwrap()
    }

    #[test]
    fn parses_credits_config_shape() {
        let config = config_of(serde_json::json!({
            "config": {
                "creditUsagePercent": 42.5,
                "currentPeriod": {
                    "type": "USAGE_PERIOD_TYPE_WEEKLY",
                    "start": "2026-06-01T00:00:00Z",
                    "end": "2026-06-08T00:00:00Z"
                },
                "prepaidBalance": {"val": 1250}
            }
        }));
        let (windows, credits) = parse_billing(&config);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].metric_id, "allowance");
        assert_eq!(windows[0].label, "Weekly allowance");
        assert_eq!(windows[0].used_pct, 42.5);
        assert_eq!(
            windows[0].resets_at,
            Some("2026-06-08T00:00:00Z".parse::<DateTime<Utc>>().unwrap())
        );
        assert_eq!(
            windows[0].period_start,
            Some("2026-06-01T00:00:00Z".parse::<DateTime<Utc>>().unwrap())
        );
        let credits = credits.unwrap();
        assert_eq!(credits.balance, 12.5);
        assert_eq!(credits.unit, "USD");
    }

    #[test]
    fn parses_legacy_shape_by_deriving_percentage() {
        let config = config_of(serde_json::json!({
            "config": {
                "monthlyLimit": {"val": 2000},
                "used": {"val": 500},
                "billingPeriodStart": "2026-04-01T00:00:00Z",
                "billingPeriodEnd": "2026-05-01T00:00:00Z"
            }
        }));
        let (windows, credits) = parse_billing(&config);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].label, "Allowance");
        assert_eq!(windows[0].used_pct, 25.0);
        assert!(windows[0].resets_at.is_some());
        // No prepaidBalance → no credits line.
        assert!(credits.is_none());
    }

    #[test]
    fn zero_prepaid_balance_is_omitted() {
        // proto3 emits a zero Cent as `{}`.
        let config = config_of(serde_json::json!({
            "config": {"creditUsagePercent": 10.0, "prepaidBalance": {}}
        }));
        let (windows, credits) = parse_billing(&config);
        assert_eq!(windows.len(), 1);
        assert!(credits.is_none(), "a $0 prepaid balance must not render");
    }

    #[test]
    fn monthly_period_label() {
        let config = config_of(serde_json::json!({
            "config": {
                "creditUsagePercent": 5.0,
                "currentPeriod": {"type": "USAGE_PERIOD_TYPE_MONTHLY", "end": "2026-07-01T00:00:00Z"}
            }
        }));
        let (windows, _) = parse_billing(&config);
        assert_eq!(windows[0].label, "Monthly allowance");
    }

    #[test]
    fn credits_only_when_no_usage_percentage() {
        // A prepaid-only account with no derivable allowance still yields a
        // credits line and no window (fetch() keeps this rather than erroring).
        let config = config_of(serde_json::json!({
            "config": {"prepaidBalance": {"val": 4200}}
        }));
        let (windows, credits) = parse_billing(&config);
        assert!(windows.is_empty());
        assert_eq!(credits.unwrap().balance, 42.0);
    }

    #[test]
    fn token_set_round_trip_and_expiry() {
        let t = GrokTokens {
            access: "a".into(),
            refresh: Some("r".into()),
            expires_at_ms: 1000,
            user_id: Some("u-1".into()),
        };
        let parsed = parse_grok_tokens(&serde_json::from_str(&t.to_secret_json()).unwrap()).unwrap();
        assert_eq!(parsed, t);
        assert!(t.expired(1_000_000));
        assert!(!t.expired(-100_000));
        // unknown expiry is treated as valid
        let t2 = GrokTokens {
            access: "a".into(),
            refresh: None,
            expires_at_ms: 0,
            user_id: None,
        };
        assert!(!t2.expired(i64::MAX - 60_000));
    }

    #[test]
    fn auth_mode_from_settings() {
        let mk = |mode: Option<&str>| {
            let mut cfg = Config::default();
            if let Some(m) = mode {
                cfg.providers
                    .get_mut("grok")
                    .unwrap()
                    .settings
                    .insert("auth_mode".into(), m.into());
            }
            ProviderCtx::new(
                std::env::temp_dir(),
                std::env::temp_dir(),
                HashMap::new(),
                cfg,
            )
        };
        assert_eq!(auth_mode(&mk(None), "grok"), AuthMode::Auto);
        assert_eq!(auth_mode(&mk(Some("cli")), "grok"), AuthMode::Cli);
        assert_eq!(auth_mode(&mk(Some("oauth")), "grok"), AuthMode::Oauth);
        assert_eq!(auth_mode(&mk(Some("bogus")), "grok"), AuthMode::Auto);
    }

    fn write_cli_auth(home: &std::path::Path, entries: serde_json::Value) {
        std::fs::create_dir_all(home.join(".grok")).unwrap();
        std::fs::write(home.join(".grok/auth.json"), entries.to_string()).unwrap();
    }

    #[test]
    fn cli_tokens_picks_most_recent_session_and_skips_api_key() {
        let dir = tempfile::tempdir().unwrap();
        write_cli_auth(
            dir.path(),
            serde_json::json!({
                "xai::api_key": {"key": "sk-should-be-ignored", "auth_mode": "api_key"},
                "https://auth.x.ai::old": {
                    "key": "old-access", "auth_mode": "oidc", "user_id": "u-old",
                    "refresh_token": "r-old", "create_time": "2026-01-01T00:00:00Z",
                    "expires_at": "2999-01-01T00:00:00Z"
                },
                "https://auth.x.ai::new": {
                    "key": "new-access", "auth_mode": "oidc", "user_id": "u-new",
                    "refresh_token": "r-new", "create_time": "2026-08-01T00:00:00Z",
                    "expires_at": "2999-01-01T00:00:00Z"
                }
            }),
        );
        let ctx = ProviderCtx::new(
            dir.path().into(),
            dir.path().into(),
            HashMap::new(),
            Config::default(),
        );
        let t = Grok::new("grok".into(), None).cli_tokens(&ctx).unwrap();
        assert_eq!(t.access, "new-access");
        assert_eq!(t.user_id.as_deref(), Some("u-new"));
    }

    #[tokio::test]
    async fn auto_prefers_fresh_cli_then_stored_secret() {
        let dir = tempfile::tempdir().unwrap();
        write_cli_auth(
            dir.path(),
            serde_json::json!({
                "https://auth.x.ai::s": {
                    "key": "cli-access", "auth_mode": "oidc", "user_id": "u-cli",
                    "expires_at": "2999-01-01T00:00:00Z"
                }
            }),
        );
        let mut secrets = HashMap::new();
        secrets.insert(
            OAUTH_SECRET_KEY.to_string(),
            r#"{"accessToken":"widget-access","expiresAt":9999999999999,"userId":"u-widget"}"#
                .to_string(),
        );
        // Fresh CLI token wins in auto mode.
        let ctx = ProviderCtx::new(
            dir.path().into(),
            dir.path().into(),
            secrets.clone(),
            Config::default(),
        );
        let t = Grok::new("grok".into(), None).resolve(&ctx).await.unwrap();
        assert_eq!(t.access, "cli-access");

        // Expired CLI token, no refresh → stored secret wins.
        write_cli_auth(
            dir.path(),
            serde_json::json!({
                "https://auth.x.ai::s": {
                    "key": "cli-access", "auth_mode": "oidc", "user_id": "u-cli",
                    "expires_at": "2000-01-01T00:00:00Z"
                }
            }),
        );
        let ctx = ProviderCtx::new(dir.path().into(), dir.path().into(), secrets, Config::default());
        let t = Grok::new("grok".into(), None).resolve(&ctx).await.unwrap();
        assert_eq!(t.access, "widget-access");
        assert_eq!(t.user_id.as_deref(), Some("u-widget"));
    }

    #[tokio::test]
    async fn oauth_mode_ignores_cli_and_is_not_configured_without_secret() {
        let dir = tempfile::tempdir().unwrap();
        write_cli_auth(
            dir.path(),
            serde_json::json!({
                "https://auth.x.ai::s": {
                    "key": "cli-access", "auth_mode": "oidc", "user_id": "u-cli",
                    "expires_at": "2999-01-01T00:00:00Z"
                }
            }),
        );
        let mut cfg = Config::default();
        cfg.providers
            .get_mut("grok")
            .unwrap()
            .settings
            .insert("auth_mode".into(), "oauth".into());
        let ctx = ProviderCtx::new(dir.path().into(), dir.path().into(), HashMap::new(), cfg);
        let err = Grok::new("grok".into(), None)
            .resolve(&ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::NotConfigured(_)));
    }
}
