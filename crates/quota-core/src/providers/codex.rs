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

pub struct Codex {
    pub key: String,
    pub label: Option<String>,
}

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

#[async_trait::async_trait]
impl Provider for Codex {
    fn kind(&self) -> &'static str {
        "codex"
    }
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        self.label.as_deref().unwrap_or("Codex")
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let auth = self.read_auth(ctx)?;
        let url = ctx
            .config
            .provider_setting(&self.key, "usage_url")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| USAGE_URL.to_string());

        let mut req = ctx
            .http
            .get(&url)
            .bearer_auth(&auth.access_token)
            .header("Accept", "application/json");
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
            return Err(FetchError::Parse(
                "no rate-limit windows in response".into(),
            ));
        }
        Ok(UsageSnapshot::ok(self.id(), self.name(), windows, None))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexAuth {
    pub access_token: String,
    pub account_id: Option<String>,
}

/// Where Codex credentials may come from, via the `auth_mode` provider
/// setting — mirrors the Claude adapter's sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    /// Codex CLI's `~/.codex/auth.json` first, then the widget's own login.
    Auto,
    /// Only the Codex CLI file.
    Cli,
    /// Only the widget's own device-code sign-in.
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

/// Tokens stored by the widget's own device-code sign-in.
pub const OAUTH_SECRET_KEY: &str = "codex_oauth";

/// Parse the `tokens` object shared by the CLI's auth.json and our own stored
/// secret: `{access_token, refresh_token, id_token, account_id}`.
pub fn parse_codex_tokens(tokens: &Value) -> Option<CodexAuth> {
    let access = tokens["access_token"].as_str().filter(|s| !s.is_empty())?;
    let account_id = tokens["account_id"]
        .as_str()
        .map(String::from)
        .or_else(|| tokens["id_token"].as_str().and_then(account_id_from_jwt));
    Some(CodexAuth {
        access_token: access.to_string(),
        account_id,
    })
}

impl Codex {
    pub fn new(key: String, label: Option<String>) -> Self {
        Self { key, label }
    }
    fn cli_auth(&self, ctx: &ProviderCtx) -> Option<CodexAuth> {
        let path = ctx.home.join(".codex").join("auth.json");
        let text = std::fs::read_to_string(&path).ok()?;
        let auth: Value = serde_json::from_str(&text).ok()?;
        parse_codex_tokens(&auth["tokens"])
    }

    fn stored_auth(&self, ctx: &ProviderCtx) -> Option<CodexAuth> {
        let raw = ctx.secrets.get(&format!("{}_oauth", self.key))?;
        let v: Value = serde_json::from_str(raw).ok()?;
        // Accept both the bare token object and a CLI-shaped wrapper.
        parse_codex_tokens(&v["tokens"]).or_else(|| parse_codex_tokens(&v))
    }

    fn read_auth(&self, ctx: &ProviderCtx) -> Result<CodexAuth, FetchError> {
        let mode = auth_mode(ctx, &self.key);
        if mode != AuthMode::Oauth {
            if let Some(auth) = self.cli_auth(ctx) {
                return Ok(auth);
            }
        }
        if mode != AuthMode::Cli {
            if let Some(auth) = self.stored_auth(ctx) {
                return Ok(auth);
            }
        }
        Err(FetchError::NotConfigured(match mode {
            AuthMode::Cli => format!(
                "no Codex CLI login found ({}) — install Codex CLI and run `codex`",
                ctx.home.join(".codex").join("auth.json").display()
            ),
            AuthMode::Oauth => {
                "not signed in — use Sign in with Codex in Settings → Codex".into()
            }
            AuthMode::Auto => {
                "no Codex login found — sign in via Settings → Codex (or run `codex` if you use the CLI)"
                    .into()
            }
        }))
    }
}

/// The ChatGPT account id lives in the id_token JWT's auth claim when
/// auth.json doesn't carry it top-level.
pub fn account_id_from_jwt(id_token: &str) -> Option<String> {
    let payload_b64 = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()?;
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
                let resets_at = obj.get("resets_at").and_then(parse_timestamp).or_else(|| {
                    obj.get("resets_in_seconds")
                        .or_else(|| obj.get("reset_after_seconds"))
                        .and_then(as_f64)
                        .map(|s| Utc::now() + Duration::seconds(s as i64))
                });
                out.push((
                    minutes,
                    UsageWindow {
                        metric_id: metric_id_for_minutes(minutes),
                        label: label_for_minutes(minutes),
                        used_pct: pct,
                        resets_at,
                        ..Default::default()
                    },
                ));
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

/// The API supplies durations rather than stable names. Normalizing the
/// duration makes selections survive response ordering and wording changes.
fn metric_id_for_minutes(minutes: f64) -> String {
    if (minutes - 10_080.0).abs() < 1_500.0 {
        "weekly".into()
    } else if (minutes - 300.0).abs() < 30.0 {
        "five_hour".into()
    } else if minutes > 0.0 {
        format!("duration_{}m", minutes.round() as i64)
    } else {
        "usage".into()
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
        assert_eq!(w[0].metric_id, "five_hour");
        assert_eq!(w[0].used_pct, 12.5);
        assert!(w[0].resets_at.is_some());
        assert_eq!(w[1].label, "Weekly");
        assert_eq!(w[1].metric_id, "weekly");
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

    fn ctx_with(home: &std::path::Path, mode: Option<&str>, secret: Option<&str>) -> ProviderCtx {
        use crate::config::Config;
        let mut cfg = Config::default();
        if let Some(m) = mode {
            cfg.providers
                .get_mut("codex")
                .unwrap()
                .settings
                .insert("auth_mode".into(), m.into());
        }
        let mut secrets = std::collections::HashMap::new();
        if let Some(s) = secret {
            secrets.insert(OAUTH_SECRET_KEY.to_string(), s.to_string());
        }
        ProviderCtx::new(home.into(), secrets, cfg)
    }

    fn write_cli_auth(home: &std::path::Path, token: &str) {
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        std::fs::write(
            home.join(".codex/auth.json"),
            serde_json::json!({
                "auth_mode": "chatgpt",
                "tokens": {"access_token": token, "account_id": "acct-cli"}
            })
            .to_string(),
        )
        .unwrap();
    }

    const STORED: &str = r#"{"tokens":{"access_token":"stored-tok","account_id":"acct-oauth"}}"#;

    #[test]
    fn auto_prefers_cli_then_falls_back_to_stored_login() {
        let dir = tempfile::tempdir().unwrap();
        // Nothing anywhere → not configured, with a hint naming both routes.
        let err = Codex::new("codex".into(), None)
            .read_auth(&ctx_with(dir.path(), None, None))
            .unwrap_err();
        assert!(matches!(err, FetchError::NotConfigured(_)));

        // Only our own login → used.
        let auth = Codex::new("codex".into(), None)
            .read_auth(&ctx_with(dir.path(), None, Some(STORED)))
            .unwrap();
        assert_eq!(auth.access_token, "stored-tok");

        // CLI login present → wins in auto mode.
        write_cli_auth(dir.path(), "cli-tok");
        let auth = Codex::new("codex".into(), None)
            .read_auth(&ctx_with(dir.path(), None, Some(STORED)))
            .unwrap();
        assert_eq!(auth.access_token, "cli-tok");
        assert_eq!(auth.account_id.as_deref(), Some("acct-cli"));
    }

    #[test]
    fn explicit_modes_ignore_the_other_source() {
        let dir = tempfile::tempdir().unwrap();
        write_cli_auth(dir.path(), "cli-tok");

        // oauth mode skips the CLI file even though it's present and valid.
        let auth = Codex::new("codex".into(), None)
            .read_auth(&ctx_with(dir.path(), Some("oauth"), Some(STORED)))
            .unwrap();
        assert_eq!(auth.access_token, "stored-tok");
        assert!(Codex::new("codex".into(), None)
            .read_auth(&ctx_with(dir.path(), Some("oauth"), None))
            .is_err());

        // cli mode ignores our stored login.
        let auth = Codex::new("codex".into(), None)
            .read_auth(&ctx_with(dir.path(), Some("cli"), Some(STORED)))
            .unwrap();
        assert_eq!(auth.access_token, "cli-tok");
        let empty = tempfile::tempdir().unwrap();
        assert!(Codex::new("codex".into(), None)
            .read_auth(&ctx_with(empty.path(), Some("cli"), Some(STORED)))
            .is_err());
    }

    /// The device flow writes the CLI's `{"tokens": {...}}` shape, but accept
    /// a bare token object too rather than silently failing to sign in.
    #[test]
    fn stored_secret_accepts_wrapped_or_bare_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let bare = r#"{"access_token":"bare-tok"}"#;
        let auth = Codex::new("codex".into(), None)
            .read_auth(&ctx_with(dir.path(), Some("oauth"), Some(bare)))
            .unwrap();
        assert_eq!(auth.access_token, "bare-tok");
    }

    #[test]
    fn account_id_extracted_from_jwt() {
        let claims = serde_json::json!({
            "https://api.openai.com/auth": {"chatgpt_account_id": "acct-123"}
        });
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        let jwt = format!("eyJhbGciOiJub25lIn0.{payload}.sig");
        assert_eq!(account_id_from_jwt(&jwt).as_deref(), Some("acct-123"));
    }
}
