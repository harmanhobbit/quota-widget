//! Claude (Pro/Max subscription) usage via the OAuth endpoint Claude Code's
//! own `/usage` command uses. Token is read from the Claude Code credential
//! file on every poll, so CLI-side refreshes are picked up automatically.
//! These are unofficial endpoints and may change without notice.

use super::{as_f64, network_err, parse_timestamp, Provider, ProviderCtx};
use crate::model::{FetchError, UsageSnapshot, UsageWindow};
use chrono::Utc;
use serde_json::Value;

pub struct Claude;

const USAGE_URLS: &[&str] = &[
    "https://claude.ai/api/oauth/usage",
    "https://api.anthropic.com/api/oauth/usage",
];
const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
/// Claude Code's public OAuth client id.
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";

#[async_trait::async_trait]
impl Provider for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }
    fn name(&self) -> &'static str {
        "Claude"
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
                    return Err(FetchError::AuthExpired(
                        "token rejected — run `claude` once to refresh the login".into(),
                    ));
                }
                Ok(r) => last_err = Some(FetchError::Network(format!("{url}: HTTP {}", r.status()))),
                Err(e) => last_err = Some(network_err(e)),
            }
        }
        Err(last_err.unwrap_or_else(|| FetchError::Network("no usage URL configured".into())))
    }
}

impl Claude {
    fn usage_urls(&self, ctx: &ProviderCtx) -> Vec<String> {
        match ctx.config.provider_setting("claude", "usage_url").and_then(|v| v.as_str().map(String::from)) {
            Some(u) => vec![u],
            None => USAGE_URLS.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Read Claude Code's credential file; refresh in memory if expired.
    async fn access_token(&self, ctx: &ProviderCtx) -> Result<String, FetchError> {
        let path = ctx.home.join(".claude").join(".credentials.json");
        let text = std::fs::read_to_string(&path).map_err(|_| {
            FetchError::NotConfigured(format!(
                "no Claude Code login found ({}) — install Claude Code and run `claude`",
                path.display()
            ))
        })?;
        let creds: Value = serde_json::from_str(&text)
            .map_err(|e| FetchError::Parse(format!("credentials file: {e}")))?;
        let oauth = &creds["claudeAiOauth"];
        let access = oauth["accessToken"].as_str().unwrap_or_default();
        if access.is_empty() {
            return Err(FetchError::NotConfigured(
                "credential file has no accessToken — run `claude` to log in".into(),
            ));
        }
        let expires_ms = oauth["expiresAt"].as_i64().unwrap_or(0);
        let now_ms = Utc::now().timestamp_millis();
        if expires_ms > 0 && expires_ms < now_ms + 60_000 {
            if let Some(refresh) = oauth["refreshToken"].as_str() {
                if let Ok(tok) = self.refresh(ctx, refresh).await {
                    return Ok(tok);
                }
            }
            return Err(FetchError::AuthExpired(
                "token expired — run `claude` once to refresh the login".into(),
            ));
        }
        Ok(access.to_string())
    }

    /// Best-effort in-memory refresh; never writes back to the CLI's file.
    async fn refresh(&self, ctx: &ProviderCtx, refresh_token: &str) -> Result<String, FetchError> {
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
            return Err(FetchError::AuthExpired(format!("refresh failed: HTTP {}", resp.status())));
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        body["access_token"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| FetchError::Parse("refresh response missing access_token".into()))
    }
}

/// The response is a flat object of window-name → {utilization, resets_at}.
/// Parse every object that looks like a window so new windows Anthropic adds
/// (e.g. per-model weekly caps) show up without a code change.
fn parse_usage(body: &Value) -> Vec<UsageWindow> {
    let Some(obj) = body.as_object() else { return vec![] };
    let mut windows = Vec::new();
    for (key, val) in obj {
        let Some(w) = val.as_object() else { continue };
        let Some(pct) = w.get("utilization").and_then(as_f64) else { continue };
        windows.push(UsageWindow {
            label: label_for(key),
            used_pct: pct,
            resets_at: w.get("resets_at").and_then(parse_timestamp),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_five_hour_and_weekly() {
        let body: Value = serde_json::from_str(
            r#"{
              "five_hour": {"utilization": 62.5, "resets_at": "2026-07-31T18:00:00Z"},
              "seven_day": {"utilization": 30, "resets_at": "2026-08-04T00:00:00Z"},
              "seven_day_opus": {"utilization": 12.0, "resets_at": null},
              "extra_field": "ignored"
            }"#,
        )
        .unwrap();
        let w = parse_usage(&body);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].label, "5-hour");
        assert_eq!(w[0].used_pct, 62.5);
        assert!(w[0].resets_at.is_some());
        assert_eq!(w[1].label, "Weekly");
        assert_eq!(w[1].used_pct, 30.0);
        assert_eq!(w[2].label, "Weekly (Opus)");
        assert_eq!(w[2].resets_at, None);
    }

    #[test]
    fn unknown_shape_yields_empty() {
        assert!(parse_usage(&serde_json::json!({"error": "nope"})).is_empty());
        assert!(parse_usage(&serde_json::json!([1, 2, 3])).is_empty());
    }
}
