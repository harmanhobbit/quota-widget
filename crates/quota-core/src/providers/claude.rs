//! Claude (Pro/Max subscription) usage via the OAuth endpoint Claude Code's
//! own `/usage` command uses. These are unofficial endpoints and may change.
//!
//! Auth sources, controlled by the `auth_mode` provider setting:
//! - `"cli"`   — only the Claude Code credential file (`~/.claude/.credentials.json`)
//! - `"oauth"` — only the widget's own sign-in (tokens stored under the
//!               `claude_oauth` secret by the host app's PKCE flow)
//! - `"auto"` (default) — a fresh CLI token wins; otherwise the widget's own
//!               stored login; otherwise a last-resort refresh with the CLI's
//!               refresh token.
//!
//! Anthropic rotates refresh tokens on use, so whenever *we* perform a refresh
//! the rotated pair is persisted to our own `claude_oauth` secret — never back
//! into Claude Code's file. That keeps the CLI's login untouched and means we
//! refresh at most once per expiry, not once per poll.

use super::{as_f64, network_err, parse_timestamp, Provider, ProviderCtx};
use crate::model::{FetchError, UsageSnapshot, UsageWindow};
use chrono::{Duration, Utc};
use serde_json::Value;

pub struct Claude {
    pub key: String,
    pub label: Option<String>,
}

pub const OAUTH_SECRET_KEY: &str = "claude_oauth";

const USAGE_URLS: &[&str] = &[
    "https://claude.ai/api/oauth/usage",
    "https://api.anthropic.com/api/oauth/usage",
];
const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
/// Claude Code's public OAuth client id.
pub const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

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

/// A usable token set from either source.
#[derive(Debug, Clone, PartialEq)]
pub struct TokenSet {
    pub access: String,
    pub refresh: Option<String>,
    /// epoch millis; 0 = unknown (treat as valid, let the API reject it)
    pub expires_at_ms: i64,
}

impl TokenSet {
    pub fn expired(&self, now_ms: i64) -> bool {
        self.expires_at_ms > 0 && self.expires_at_ms < now_ms + 60_000
    }

    pub fn to_secret_json(&self) -> String {
        serde_json::json!({
            "accessToken": self.access,
            "refreshToken": self.refresh,
            "expiresAt": self.expires_at_ms,
        })
        .to_string()
    }
}

/// Parse either our own stored secret or Claude Code's `claudeAiOauth` object —
/// both use accessToken/refreshToken/expiresAt.
pub fn parse_token_set(v: &Value) -> Option<TokenSet> {
    let access = v["accessToken"].as_str()?.to_string();
    if access.is_empty() {
        return None;
    }
    Some(TokenSet {
        access,
        refresh: v["refreshToken"].as_str().map(String::from),
        expires_at_ms: v["expiresAt"].as_i64().unwrap_or(0),
    })
}

#[async_trait::async_trait]
impl Provider for Claude {
    fn kind(&self) -> &'static str {
        "claude"
    }
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        self.label.as_deref().unwrap_or("Claude")
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let token = self.access_token(ctx).await?;
        let mut last_err = None;
        for url in self.usage_urls(ctx) {
            let resp = ctx
                .http
                .get(&url)
                .bearer_auth(&token)
                .header("anthropic-beta", "oauth-2025-04-20")
                .header("Accept", "application/json")
                .send()
                .await;
            match resp {
                Ok(r) if r.status().is_success() => {
                    let body: Value = r.json().await.map_err(network_err)?;
                    let windows = parse_usage(&body);
                    if windows.is_empty() {
                        return Err(FetchError::Parse(format!(
                            "no usage windows in response from {url}"
                        )));
                    }
                    return Ok(UsageSnapshot::ok(self.id(), self.name(), windows, None));
                }
                Ok(r) if r.status().as_u16() == 401 || r.status().as_u16() == 403 => {
                    return Err(FetchError::AuthExpired(reauth_hint(auth_mode(
                        ctx, &self.key,
                    ))));
                }
                Ok(r) => {
                    last_err = Some(FetchError::Network(format!("{url}: HTTP {}", r.status())))
                }
                Err(e) => last_err = Some(network_err(e)),
            }
        }
        Err(last_err.unwrap_or_else(|| FetchError::Network("no usage URL configured".into())))
    }
}

fn reauth_hint(mode: AuthMode) -> String {
    match mode {
        AuthMode::Cli => "token rejected — run `claude` once to refresh the login".into(),
        AuthMode::Oauth => "token rejected — sign in again in Settings → Claude".into(),
        AuthMode::Auto => {
            "token rejected — sign in via Settings → Claude (or run `claude` if you use the CLI)"
                .into()
        }
    }
}

fn notconf_hint(mode: AuthMode) -> String {
    match mode {
        AuthMode::Cli => "no Claude Code login found — install Claude Code and run `claude`".into(),
        AuthMode::Oauth => "not signed in — use Settings → Claude → Sign in".into(),
        AuthMode::Auto => {
            "no Claude login found — sign in via Settings → Claude (or run `claude` if you use the CLI)"
                .into()
        }
    }
}

impl Claude {
    pub fn new(key: String, label: Option<String>) -> Self {
        Self { key, label }
    }
    fn secret_key(&self) -> String {
        format!("{}_oauth", self.key)
    }
    fn usage_urls(&self, ctx: &ProviderCtx) -> Vec<String> {
        match ctx
            .config
            .provider_setting(&self.key, "usage_url")
            .and_then(|v| v.as_str().map(String::from))
        {
            Some(u) => vec![u],
            None => USAGE_URLS.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn cli_tokens(&self, ctx: &ProviderCtx) -> Option<TokenSet> {
        let path = ctx.home.join(".claude").join(".credentials.json");
        let text = std::fs::read_to_string(&path).ok()?;
        let creds: Value = serde_json::from_str(&text).ok()?;
        parse_token_set(&creds["claudeAiOauth"])
    }

    fn stored_tokens(&self, ctx: &ProviderCtx) -> Option<TokenSet> {
        let raw = ctx.secrets.get(&self.secret_key())?;
        parse_token_set(&serde_json::from_str(raw).ok()?)
    }

    async fn access_token(&self, ctx: &ProviderCtx) -> Result<String, FetchError> {
        let mode = auth_mode(ctx, &self.key);
        let now_ms = Utc::now().timestamp_millis();

        // 1. A fresh token from the Claude Code CLI file (kept current by the
        //    CLI itself whenever the user runs it).
        let cli = if mode != AuthMode::Oauth {
            self.cli_tokens(ctx)
        } else {
            None
        };
        if let Some(t) = &cli {
            if !t.expired(now_ms) {
                return Ok(t.access.clone());
            }
        }

        // 2. Our own stored login, refreshed (and re-persisted) as needed.
        if mode != AuthMode::Cli {
            if let Some(t) = self.stored_tokens(ctx) {
                if !t.expired(now_ms) {
                    return Ok(t.access);
                }
                if let Some(refresh) = &t.refresh {
                    if let Ok(fresh) = self.refresh(ctx, refresh).await {
                        ctx.persist_secret(&self.secret_key(), &fresh.to_secret_json());
                        return Ok(fresh.access);
                    }
                }
                return Err(FetchError::AuthExpired(reauth_hint(mode)));
            }
        }

        // 3. Last resort: refresh with the CLI's refresh token. The rotated
        //    pair is stored under our secret, not written back to the CLI file.
        if let Some(t) = cli {
            if let Some(refresh) = &t.refresh {
                if let Ok(fresh) = self.refresh(ctx, refresh).await {
                    ctx.persist_secret(&self.secret_key(), &fresh.to_secret_json());
                    return Ok(fresh.access);
                }
            }
            return Err(FetchError::AuthExpired(reauth_hint(mode)));
        }

        Err(FetchError::NotConfigured(notconf_hint(mode)))
    }

    async fn refresh(
        &self,
        ctx: &ProviderCtx,
        refresh_token: &str,
    ) -> Result<TokenSet, FetchError> {
        let resp = ctx
            .http
            .post(TOKEN_URL)
            .json(&serde_json::json!({
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
                "client_id": CLIENT_ID,
            }))
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
        Ok(TokenSet {
            access,
            // Anthropic rotates refresh tokens; fall back to the one we used
            // if the response doesn't include a new one.
            refresh: body["refresh_token"]
                .as_str()
                .map(String::from)
                .or_else(|| Some(refresh_token.into())),
            expires_at_ms,
        })
    }
}

/// The response is a flat object of window-name → {utilization, resets_at}.
/// Parse every object that looks like a window so new windows Anthropic adds
/// (e.g. per-model weekly caps) show up without a code change.
fn parse_usage(body: &Value) -> Vec<UsageWindow> {
    let Some(obj) = body.as_object() else {
        return vec![];
    };
    let mut windows = Vec::new();
    for (key, val) in obj {
        let Some(w) = val.as_object() else { continue };
        let Some(pct) = w.get("utilization").and_then(as_f64) else {
            continue;
        };
        let resets_at = w.get("resets_at").and_then(parse_timestamp);
        windows.push(UsageWindow {
            metric_id: metric_id_for(key),
            label: label_for(key),
            used_pct: pct,
            resets_at,
            period_start: resets_at.and_then(|r| period_len_for(key).map(|len| r - len)),
            ..Default::default()
        });
    }
    // Stable, human-sensible order: 5-hour first, then weekly, then the rest.
    windows.sort_by_key(|w| match w.label.as_str() {
        "5-hour" => 0,
        "Weekly" => 1,
        _ => 2,
    });
    windows
}

fn label_for(key: &str) -> String {
    match key {
        "five_hour" => "5-hour".into(),
        "seven_day" => "Weekly".into(),
        "seven_day_opus" => "Weekly (Opus)".into(),
        "seven_day_sonnet" => "Weekly (Sonnet)".into(),
        "seven_day_oauth_apps" => "Weekly (apps)".into(),
        other => other.replace('_', " "),
    }
}

/// Window length implied by the key, for the period-progress marker. Anthropic
/// doesn't report it, but the names are the duration: anything `seven_day_*` is
/// a week regardless of which model it caps. Unknown keys get no marker rather
/// than a guessed one.
fn period_len_for(key: &str) -> Option<Duration> {
    match key {
        "five_hour" => Some(Duration::hours(5)),
        k if k == "seven_day" || k.starts_with("seven_day_") => Some(Duration::days(7)),
        _ => None,
    }
}

fn metric_id_for(key: &str) -> String {
    match key {
        "five_hour" => "five_hour".into(),
        "seven_day" => "weekly".into(),
        other => other
            .strip_prefix("seven_day_")
            .map(|model| format!("weekly_{model}"))
            .unwrap_or_else(|| other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use chrono::DateTime;
    use std::collections::HashMap;

    #[test]
    fn parses_five_hour_and_weekly() {
        let body: Value = serde_json::from_str(
            r#"{
              "five_hour": {"utilization": 62.5, "resets_at": "2026-07-31T18:00:00Z"},
              "seven_day": {"utilization": 30, "resets_at": "2026-08-04T00:00:00Z"},
              "seven_day_opus": {"utilization": 12.0, "resets_at": null},
              "mystery_window": {"utilization": 5.0, "resets_at": "2026-08-04T00:00:00Z"},
              "extra_field": "ignored"
            }"#,
        )
        .unwrap();
        let w = parse_usage(&body);
        assert_eq!(w.len(), 4);
        assert_eq!(w[0].label, "5-hour");
        assert_eq!(w[0].metric_id, "five_hour");
        assert_eq!(w[0].used_pct, 62.5);
        assert!(w[0].resets_at.is_some());
        assert_eq!(w[1].label, "Weekly");
        assert_eq!(w[1].used_pct, 30.0);
        assert_eq!(w[2].label, "Weekly (Opus)");
        assert_eq!(w[1].metric_id, "weekly");
        assert_eq!(w[2].metric_id, "weekly_opus");
        assert_eq!(w[2].resets_at, None);

        // The period start is the reset minus the length the key implies.
        assert_eq!(
            w[0].period_start,
            Some("2026-07-31T13:00:00Z".parse::<DateTime<Utc>>().unwrap())
        );
        assert_eq!(
            w[1].period_start,
            Some("2026-07-28T00:00:00Z".parse::<DateTime<Utc>>().unwrap())
        );
        // No reset time, so nothing to measure a period against.
        assert_eq!(w[2].period_start, None);
        // Known reset, but an unrecognised key: no length to subtract, so no
        // marker rather than a guessed one.
        let mystery = w.iter().find(|w| w.metric_id == "mystery_window").unwrap();
        assert!(mystery.resets_at.is_some());
        assert_eq!(mystery.period_start, None);
    }

    #[test]
    fn unknown_shape_yields_empty() {
        assert!(parse_usage(&serde_json::json!({"error": "nope"})).is_empty());
        assert!(parse_usage(&serde_json::json!([1, 2, 3])).is_empty());
    }

    #[test]
    fn token_set_round_trip_and_expiry() {
        let t = TokenSet {
            access: "a".into(),
            refresh: Some("r".into()),
            expires_at_ms: 1000,
        };
        let parsed = parse_token_set(&serde_json::from_str(&t.to_secret_json()).unwrap()).unwrap();
        assert_eq!(parsed, t);
        assert!(t.expired(1_000_000));
        assert!(!t.expired(-100_000));
        // unknown expiry is treated as valid
        let t2 = TokenSet {
            access: "a".into(),
            refresh: None,
            expires_at_ms: 0,
        };
        assert!(!t2.expired(i64::MAX - 60_000));
    }

    #[test]
    fn auth_mode_from_settings() {
        let mk = |mode: Option<&str>| {
            let mut cfg = Config::default();
            if let Some(m) = mode {
                cfg.providers
                    .get_mut("claude")
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
        assert_eq!(auth_mode(&mk(None), "claude"), AuthMode::Auto);
        assert_eq!(auth_mode(&mk(Some("cli")), "claude"), AuthMode::Cli);
        assert_eq!(auth_mode(&mk(Some("oauth")), "claude"), AuthMode::Oauth);
        assert_eq!(auth_mode(&mk(Some("bogus")), "claude"), AuthMode::Auto);
    }

    #[tokio::test]
    async fn oauth_mode_without_secret_is_not_configured() {
        let mut cfg = Config::default();
        cfg.providers
            .get_mut("claude")
            .unwrap()
            .settings
            .insert("auth_mode".into(), "oauth".into());
        // home dir with a CLI credential file that must be IGNORED in oauth mode
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(
            dir.path().join(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"cli-token","expiresAt":9999999999999}}"#,
        )
        .unwrap();
        let ctx = ProviderCtx::new(dir.path().into(), dir.path().into(), HashMap::new(), cfg);
        let err = Claude::new("claude".into(), None)
            .access_token(&ctx)
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::NotConfigured(_)));
    }

    #[tokio::test]
    async fn auto_mode_prefers_fresh_cli_then_stored_secret() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(
            dir.path().join(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"cli-token","expiresAt":9999999999999}}"#,
        )
        .unwrap();
        let mut secrets = HashMap::new();
        secrets.insert(
            OAUTH_SECRET_KEY.to_string(),
            r#"{"accessToken":"widget-token","expiresAt":9999999999999}"#.to_string(),
        );
        // fresh CLI token wins
        let ctx = ProviderCtx::new(
            dir.path().into(),
            dir.path().into(),
            secrets.clone(),
            Config::default(),
        );
        assert_eq!(
            Claude::new("claude".into(), None)
                .access_token(&ctx)
                .await
                .unwrap(),
            "cli-token"
        );
        // expired CLI token without refresh → stored secret wins
        std::fs::write(
            dir.path().join(".claude/.credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"cli-token","expiresAt":1}}"#,
        )
        .unwrap();
        let ctx = ProviderCtx::new(
            dir.path().into(),
            dir.path().into(),
            secrets,
            Config::default(),
        );
        assert_eq!(
            Claude::new("claude".into(), None)
                .access_token(&ctx)
                .await
                .unwrap(),
            "widget-token"
        );
    }
}
