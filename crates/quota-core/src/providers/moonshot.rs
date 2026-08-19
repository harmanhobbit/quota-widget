//! Moonshot has two distinct products that deliberately keep their auth and
//! reporting paths separate: an Open Platform API key reads monetary balance,
//! while Kimi Code OAuth reads subscription rate limits. The Kimi Code flow and
//! endpoint are taken from Moonshot's published Kimi Code CLI.

use super::{as_f64, network_err, parse_timestamp, require_https, Provider, ProviderCtx};
use crate::model::{Credits, FetchError, UsageSnapshot, UsageWindow};
use chrono::{Duration, Utc};
use serde_json::Value;

const BALANCE_URL: &str = "https://api.moonshot.ai/v1/users/me/balance";
const KIMI_USAGE_URL: &str = "https://api.kimi.com/coding/v1/usages";
const KIMI_TOKEN_URL: &str = "https://auth.kimi.com/api/oauth/token";
/// Public client id used by Moonshot's published Kimi Code CLI.
pub const KIMI_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";

pub struct Moonshot {
    pub key: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    ApiKey,
    KimiCode,
}

pub fn auth_mode(ctx: &ProviderCtx, key: &str) -> AuthMode {
    match ctx
        .config
        .provider_setting(key, "auth_mode")
        .and_then(|v| v.as_str().map(str::to_owned))
        .as_deref()
    {
        Some("kimi_code") => AuthMode::KimiCode,
        _ => AuthMode::ApiKey,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct KimiTokens {
    pub access: String,
    pub refresh: String,
    /// Unix seconds; zero is unknown and lets the API decide.
    pub expires_at: i64,
    pub expires_in: i64,
}

pub fn parse_kimi_tokens(v: &Value) -> Option<KimiTokens> {
    let access = v["access_token"].as_str()?.trim();
    let refresh = v["refresh_token"].as_str()?.trim();
    if access.is_empty() || refresh.is_empty() {
        return None;
    }
    Some(KimiTokens {
        access: access.to_owned(),
        refresh: refresh.to_owned(),
        expires_at: v["expires_at"].as_i64().unwrap_or(0),
        expires_in: v["expires_in"].as_i64().unwrap_or(0),
    })
}

impl KimiTokens {
    fn expired(&self) -> bool {
        let threshold = self.expires_in.div_euclid(2).max(300);
        self.expires_at > 0 && self.expires_at < Utc::now().timestamp() + threshold
    }

    fn to_secret_json(&self) -> String {
        serde_json::json!({
            "access_token": self.access,
            "refresh_token": self.refresh,
            "expires_at": self.expires_at,
            "expires_in": self.expires_in,
        })
        .to_string()
    }
}

impl Moonshot {
    pub fn new(key: String, label: Option<String>) -> Self {
        Self { key, label }
    }

    fn oauth_key(&self) -> String {
        format!("{}_oauth", self.key)
    }

    fn stored_kimi_tokens(&self, ctx: &ProviderCtx) -> Option<KimiTokens> {
        let raw = ctx.secrets.get(&self.oauth_key())?;
        parse_kimi_tokens(&serde_json::from_str(raw).ok()?)
    }

    async fn kimi_tokens(&self, ctx: &ProviderCtx) -> Result<KimiTokens, FetchError> {
        let tokens = self.stored_kimi_tokens(ctx).ok_or_else(|| {
            FetchError::NotConfigured(
                "not signed in — use Settings → Moonshot → Sign in with Kimi Code".into(),
            )
        })?;
        if !tokens.expired() {
            return Ok(tokens);
        }
        let fresh = self.refresh_kimi(ctx, &tokens).await?;
        ctx.persist_secret(&self.oauth_key(), &fresh.to_secret_json());
        Ok(fresh)
    }

    async fn refresh_kimi(
        &self,
        ctx: &ProviderCtx,
        old: &KimiTokens,
    ) -> Result<KimiTokens, FetchError> {
        let resp = ctx
            .http
            .post(KIMI_TOKEN_URL)
            .form(&[
                ("client_id", KIMI_CLIENT_ID),
                ("grant_type", "refresh_token"),
                ("refresh_token", old.refresh.as_str()),
            ])
            .send()
            .await
            .map_err(network_err)?;
        if !resp.status().is_success() {
            return Err(FetchError::AuthExpired(
                "Kimi Code sign-in expired — sign in again in Settings → Moonshot".into(),
            ));
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        let access = body["access_token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                FetchError::Parse("Kimi refresh response missing access_token".into())
            })?;
        let refresh = body["refresh_token"]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                FetchError::Parse("Kimi refresh response missing refresh_token".into())
            })?;
        let expires_in = body["expires_in"]
            .as_i64()
            .filter(|seconds| *seconds > 0)
            .ok_or_else(|| FetchError::Parse("Kimi refresh response missing expires_in".into()))?;
        Ok(KimiTokens {
            access: access.into(),
            refresh: refresh.into(),
            expires_at: Utc::now().timestamp() + expires_in,
            expires_in,
        })
    }

    async fn fetch_balance(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let key = ctx
            .secrets
            .get(&self.key)
            .filter(|key| !key.is_empty())
            .ok_or_else(|| {
                FetchError::NotConfigured("paste a Moonshot API key in Settings".into())
            })?;
        let url = ctx
            .config
            .provider_setting(&self.key, "balance_url")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| BALANCE_URL.into());
        require_https(&url)?;
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
            status => {
                return Err(FetchError::Network(format!(
                    "HTTP {status} from balance endpoint"
                )))
            }
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        let credits = parse_credits(&body)
            .ok_or_else(|| FetchError::Parse("balance response missing totals".into()))?;
        Ok(UsageSnapshot::ok(
            self.id(),
            self.name(),
            vec![],
            Some(credits),
        ))
    }

    async fn fetch_kimi_usage(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let tokens = self.kimi_tokens(ctx).await?;
        let url = ctx
            .config
            .provider_setting(&self.key, "usage_url")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| KIMI_USAGE_URL.into());
        require_https(&url)?;
        let resp = ctx
            .http
            .get(&url)
            .bearer_auth(tokens.access)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(network_err)?;
        match resp.status().as_u16() {
            200..=299 => {}
            401 | 403 => {
                return Err(FetchError::AuthExpired(
                    "Kimi Code sign-in rejected — sign in again in Settings → Moonshot".into(),
                ))
            }
            status => {
                return Err(FetchError::Network(format!(
                    "HTTP {status} from Kimi Code usage endpoint"
                )))
            }
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        let windows = parse_kimi_usage(&body);
        if windows.is_empty() {
            return Err(FetchError::Parse(
                "Kimi Code usage response contained no limits".into(),
            ));
        }
        Ok(UsageSnapshot::ok(self.id(), self.name(), windows, None))
    }
}

#[async_trait::async_trait]
impl Provider for Moonshot {
    fn kind(&self) -> &'static str {
        "moonshot"
    }
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        self.label.as_deref().unwrap_or("Moonshot")
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        match auth_mode(ctx, &self.key) {
            AuthMode::ApiKey => self.fetch_balance(ctx).await,
            AuthMode::KimiCode => self.fetch_kimi_usage(ctx).await,
        }
    }
}

/// `available_balance` is the figure that actually gates Open Platform calls.
fn parse_credits(body: &Value) -> Option<Credits> {
    let data = body.get("data").unwrap_or(body);
    let balance = data.get("available_balance").and_then(as_f64)?;
    let cash = data.get("cash_balance").and_then(as_f64);
    let voucher = data.get("voucher_balance").and_then(as_f64);
    Some(Credits {
        balance,
        label: None,
        unit: "USD".into(),
        used: None,
        granted: match (cash, voucher) {
            (None, None) => None,
            (cash, voucher) => Some(cash.unwrap_or(0.0) + voucher.unwrap_or(0.0)),
        },
        est_tokens_remaining: None,
    })
}

fn parse_kimi_usage(body: &Value) -> Vec<UsageWindow> {
    let mut rows = Vec::new();
    if let Some(summary) = kimi_usage_row(body.get("usage"), Some((1.0, "week")), Some("Weekly")) {
        rows.push(summary);
    }
    if let Some(limits) = body.get("limits").and_then(Value::as_array) {
        for item in limits {
            let window = item.get("window").and_then(kimi_window);
            let label = item.get("name").and_then(Value::as_str);
            if let Some(row) = kimi_usage_row(item.get("detail"), window, label) {
                rows.push(row);
            }
        }
    }
    rows
}

fn kimi_usage_row(
    detail: Option<&Value>,
    window: Option<(f64, &str)>,
    name: Option<&str>,
) -> Option<UsageWindow> {
    let detail = detail?;
    let used = detail.get("used").and_then(as_f64).unwrap_or(0.0);
    let limit = detail.get("limit").and_then(as_f64)?;
    if limit <= 0.0 {
        return None;
    }
    let (duration, unit) = window.unwrap_or((0.0, ""));
    let label = name
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if duration == 1.0 && unit == "week" {
                "Weekly".into()
            } else if duration > 0.0 {
                format_duration(duration, unit)
            } else {
                "Usage".into()
            }
        });
    let resets_at = detail.get("resetTime").and_then(parse_timestamp);
    let seconds = duration_seconds(duration, unit);
    Some(UsageWindow {
        metric_id: metric_id(&label, duration, unit),
        label,
        used_pct: (used / limit * 100.0).clamp(0.0, 100.0),
        resets_at,
        period_start: resets_at
            .zip(seconds)
            .map(|(reset, seconds)| reset - Duration::seconds(seconds)),
        ..Default::default()
    })
}

fn kimi_window(value: &Value) -> Option<(f64, &str)> {
    let duration = value.get("duration").and_then(as_f64)?;
    let unit = match value.get("timeUnit").and_then(Value::as_str)? {
        "TIME_UNIT_MINUTE" => "minute",
        "TIME_UNIT_HOUR" => "hour",
        "TIME_UNIT_DAY" => "day",
        "TIME_UNIT_WEEK" => "week",
        _ => return None,
    };
    if unit == "minute" && duration >= 60.0 && duration % 60.0 == 0.0 {
        return Some((duration / 60.0, "hour"));
    }
    Some((duration, unit))
}

fn duration_seconds(duration: f64, unit: &str) -> Option<i64> {
    let multiplier = match unit {
        "minute" => 60.0,
        "hour" => 3600.0,
        "day" => 86_400.0,
        "week" => 604_800.0,
        _ => return None,
    };
    Some((duration * multiplier) as i64)
}

fn format_duration(duration: f64, unit: &str) -> String {
    let rendered = if duration.fract() == 0.0 {
        format!("{duration:.0}")
    } else {
        duration.to_string()
    };
    format!("{rendered}-{unit}")
}

fn metric_id(label: &str, duration: f64, unit: &str) -> String {
    if duration == 1.0 && unit == "week" {
        return "weekly".into();
    }
    if duration == 5.0 && unit == "hour" {
        return "five_hour".into();
    }
    let normalized: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("kimi_{}", normalized.trim_matches('_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        serde_json::json!({
            "code": 0,
            "data": {
                "available_balance": 49.58894,
                "voucher_balance": 46.58893,
                "cash_balance": 3.00001
            },
            "scode": "0x0",
            "status": true
        })
    }

    #[test]
    fn parses_documented_shape() {
        let c = parse_credits(&sample()).unwrap();
        assert!((c.balance - 49.58894).abs() < 1e-9);
        assert_eq!(c.unit, "USD");
        // Cash and voucher fund the available total.
        assert!((c.granted.unwrap() - 49.58894).abs() < 1e-5);
        assert_eq!(c.used, None);
    }

    #[test]
    fn exhausted_balance_is_zero_not_missing() {
        let mut body = sample();
        body["data"]["available_balance"] = serde_json::json!(0);
        assert_eq!(parse_credits(&body).unwrap().balance, 0.0);
    }

    #[test]
    fn negative_balance_survives() {
        let mut body = sample();
        body["data"]["available_balance"] = serde_json::json!(-1.5);
        assert!(parse_credits(&body).unwrap().balance < 0.0);
    }

    #[test]
    fn missing_available_balance_is_none() {
        assert!(parse_credits(&serde_json::json!({"data": {"cash_balance": 1}})).is_none());
        assert!(parse_credits(&serde_json::json!({"code": 0})).is_none());
    }

    #[test]
    fn parses_kimi_code_weekly_and_five_hour_limits() {
        let body = serde_json::json!({
            "usage": {"used": "40", "limit": "1000", "resetTime": "2026-08-03T05:20:51Z"},
            "limits": [{
                "window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"},
                "detail": {"used": "1", "limit": "100", "resetTime": "2026-08-03T05:20:51Z"}
            }]
        });
        let usage = parse_kimi_usage(&body);
        assert_eq!(usage.len(), 2);
        assert_eq!(usage[0].metric_id, "weekly");
        assert_eq!(usage[0].used_pct, 4.0);
        assert_eq!(usage[1].metric_id, "five_hour");
        assert_eq!(usage[1].used_pct, 1.0);
        assert_eq!(
            usage[1].resets_at.unwrap() - usage[1].period_start.unwrap(),
            Duration::hours(5)
        );
    }

    #[test]
    fn token_parser_rejects_partial_tokens() {
        assert!(parse_kimi_tokens(&serde_json::json!({"access_token": "a"})).is_none());
        assert!(
            parse_kimi_tokens(&serde_json::json!({"access_token": "a", "refresh_token": "r"}))
                .is_some()
        );
    }
}
