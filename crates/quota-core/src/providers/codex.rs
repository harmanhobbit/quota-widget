//! Codex (ChatGPT plan) usage via the backend endpoint the Codex CLI's
//! `/status` uses, authenticated with the tokens in `~/.codex/auth.json`.
//! OpenAI has reshaped this response before (and dropped the 5-hour window),
//! so parsing is deliberately schema-tolerant: it renders whatever rate-limit
//! windows the response actually contains.

use super::{as_f64, network_err, parse_timestamp, Provider, ProviderCtx};
use crate::model::{FetchError, UsageSnapshot, UsageWindow};
use base64::Engine;
use chrono::{Duration, Utc};
use serde_json::Value;

pub struct Codex;

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

#[async_trait::async_trait]
impl Provider for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn name(&self) -> &'static str {
        "Codex"
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let auth = self.read_auth(ctx)?;
        let url = ctx
            .config
            .provider_setting("codex", "usage_url")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| USAGE_URL.to_string());

        let mut req = ctx.http.get(&url).bearer_auth(&auth.access_token).header("Accept", "application/json");
        if let Some(acct) = &auth.account_id {
            req = req.header("chatgpt-account-id", acct);
        }
        let resp = req.send().await.map_err(network_err)?;
        match resp.status().as_u16() {
            200..=299 => {}
            401 | 403 => {
                return Err(FetchError::AuthExpired(
                    "token rejected — run `codex` once to refresh the login".into(),
                ))
            }
            s => return Err(FetchError::Network(format!("HTTP {s} from usage endpoint"))),
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        let windows = parse_usage(&body);
        if windows.is_empty() {
            return Err(FetchError::Parse("no rate-limit windows in response".into()));
        }
        Ok(UsageSnapshot::ok(self.id(), self.name(), windows, None))
    }
}

struct CodexAuth {
    access_token: String,
    account_id: Option<String>,
}

impl Codex {
    fn read_auth(&self, ctx: &ProviderCtx) -> Result<CodexAuth, FetchError> {
        let path = ctx.home.join(".codex").join("auth.json");
        let text = std::fs::read_to_string(&path).map_err(|_| {
            FetchError::NotConfigured(format!(
                "no Codex CLI login found ({}) — install Codex CLI and run `codex`",
                path.display()
            ))
        })?;
        let auth: Value =
            serde_json::from_str(&text).map_err(|e| FetchError::Parse(format!("auth.json: {e}")))?;
        let tokens = &auth["tokens"];
        let access = tokens["access_token"].as_str().unwrap_or_default();
        if access.is_empty() {
            return Err(FetchError::NotConfigured(
                "auth.json has no access_token — run `codex` to log in".into(),
            ));
        }
        let account_id = tokens["account_id"]
            .as_str()
            .map(String::from)
            .or_else(|| tokens["id_token"].as_str().and_then(account_id_from_jwt));
        Ok(CodexAuth { access_token: access.to_string(), account_id })
    }
}

/// The ChatGPT account id lives in the id_token JWT's auth claim when
/// auth.json doesn't carry it top-level.
fn account_id_from_jwt(id_token: &str) -> Option<String> {
    let payload_b64 = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims["https://api.openai.com/auth"]["chatgpt_account_id"]
        .as_str()
        .map(String::from)
}

/// Recursively collect every object carrying a `used_percent` field, labelling
/// each by its window length. Handles both the `rate_limits.{primary,secondary}`
/// shape and older/flatter variants.
fn parse_usage(body: &Value) -> Vec<UsageWindow> {
    let mut found = Vec::new();
    collect_windows(body, &mut found);
    // Dedupe identical windows (some responses repeat the block), keep order:
    // shortest window first.
    found.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    found.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    found.into_iter().map(|(_, w)| w).collect()
}

fn collect_windows(v: &Value, out: &mut Vec<(f64, UsageWindow)>) {
    match v {
        Value::Object(obj) => {
            let pct = obj.get("used_percent").and_then(as_f64);
            if let Some(pct) = pct {
                let minutes = obj
                    .get("window_minutes")
                    .or_else(|| obj.get("window_duration_mins"))
                    .and_then(as_f64)
                    .unwrap_or(0.0);
                let resets_at = obj
                    .get("resets_at")
                    .and_then(parse_timestamp)
                    .or_else(|| {
                        obj.get("resets_in_seconds")
                            .or_else(|| obj.get("reset_after_seconds"))
                            .and_then(as_f64)
                            .map(|s| Utc::now() + Duration::seconds(s as i64))
                    });
                out.push((minutes, UsageWindow { label: label_for_minutes(minutes), used_pct: pct, resets_at }));
            } else {
                for val in obj.values() {
                    collect_windows(val, out);
                }
            }
        }
        Value::Array(arr) => {
            for val in arr {
                collect_windows(val, out);
            }
        }
        _ => {}
    }
}

fn label_for_minutes(minutes: f64) -> String {
    if minutes <= 0.0 {
        "Usage".into()
    } else if minutes <= 360.0 {
        format!("{:.0}-hour", minutes / 60.0)
    } else if (minutes - 10080.0).abs() < 1500.0 {
        "Weekly".into()
    } else {
        format!("{:.0}-day", minutes / 1440.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_primary_secondary_shape() {
        let body: Value = serde_json::from_str(
            r#"{
              "plan_type": "plus",
              "rate_limits": {
                "primary":   {"used_percent": 12.5, "window_minutes": 300,   "resets_in_seconds": 3600},
                "secondary": {"used_percent": 44.0, "window_minutes": 10080, "resets_in_seconds": 200000}
              }
            }"#,
        )
        .unwrap();
        let w = parse_usage(&body);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].label, "5-hour");
        assert_eq!(w[0].used_pct, 12.5);
        assert!(w[0].resets_at.is_some());
        assert_eq!(w[1].label, "Weekly");
    }

    #[test]
    fn weekly_only_shape_after_5h_removal() {
        let body: Value = serde_json::from_str(
            r#"{"rate_limit": {"used_percent": 71, "window_minutes": 10080, "resets_at": "2026-08-03T09:00:00Z"}}"#,
        )
        .unwrap();
        let w = parse_usage(&body);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].label, "Weekly");
        assert_eq!(w[0].used_pct, 71.0);
    }

    #[test]
    fn no_windows_yields_empty() {
        assert!(parse_usage(&serde_json::json!({"plan_type": "plus"})).is_empty());
    }

    #[test]
    fn account_id_extracted_from_jwt() {
        let claims = serde_json::json!({
            "https://api.openai.com/auth": {"chatgpt_account_id": "acct-123"}
        });
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        let jwt = format!("eyJhbGciOiJub25lIn0.{payload}.sig");
        assert_eq!(account_id_from_jwt(&jwt).as_deref(), Some("acct-123"));
    }
}
