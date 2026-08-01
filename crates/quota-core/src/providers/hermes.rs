//! Hermes Portal (Nous Research) credits.
//!
//! Preferred source: the hermes-agent login in `~/.hermes/auth.json`
//! (`providers.nous.access_token`), used against the portal's own billing API:
//! `GET {portal}/api/billing/state` → `balanceUsd`, `monthlyCap`, …
//! (contract confirmed from hermes-agent's client code and test fixtures).
//!
//! IMPORTANT: we only ever use the *access* token, never hermes's refresh
//! token. The portal rotates refresh tokens and treats reuse as token theft,
//! revoking the whole session chain — hermes-agent's own source documents
//! this failure mode. Access tokens are short-lived (~1 h) and refreshed by
//! hermes's keepalive, so a stale token here means "run hermes", not "refresh
//! it ourselves".
//!
//! Fallback source: a pasted portal session cookie against a user-configured
//! endpoint (`endpoint` setting), parsed leniently — kept for machines without
//! hermes-agent installed. The `source` setting forces one path:
//! `"auto"` (default) | `"hermes"` | `"cookie"`.

use super::{as_f64, network_err, Provider, ProviderCtx};
use crate::model::{Credits, FetchError, UsageSnapshot, UsageWindow};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::Path;

pub struct Hermes {
    pub key: String,
    pub label: Option<String>,
}

const DEFAULT_PORTAL: &str = "https://portal.nousresearch.com";

#[async_trait::async_trait]
impl Provider for Hermes {
    fn kind(&self) -> &'static str {
        "hermes"
    }
    fn id(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        self.label.as_deref().unwrap_or("Hermes Portal")
    }

    async fn fetch(&self, ctx: &ProviderCtx) -> Result<UsageSnapshot, FetchError> {
        let source = ctx
            .config
            .provider_setting(&self.key, "source")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "auto".into());
        let token_price = ctx
            .config
            .provider_setting(&self.key, "token_price")
            .and_then(|v| as_f64(&v));

        // Auth candidates in preference order; remember expired ones so the
        // error is "expired" (actionable) rather than "not configured".
        let mut expired_hint: Option<String> = None;

        if matches!(source.as_str(), "auto" | "hermes") {
            if let Some(mut auth) = read_hermes_auth(&ctx.home) {
                if !auth.is_fresh() {
                    // Ask the local hermes CLI to refresh its own login (it
                    // persists the rotated tokens safely), then re-read.
                    if run_local_refresh(ctx, &self.key).await {
                        if let Some(a) = read_hermes_auth(&ctx.home) {
                            auth = a;
                        }
                    }
                }
                if auth.is_fresh() {
                    return self.fetch_billing(ctx, &auth, token_price).await;
                }
                expired_hint = Some(
                    "hermes-agent token expired and auto-refresh failed — run `hermes auth status nous` by hand"
                        .into(),
                );
            } else if source == "hermes" {
                return Err(FetchError::NotConfigured(
                    "no hermes-agent login found (~/.hermes/auth.json) — run `hermes` and sign in"
                        .into(),
                ));
            }
        }

        if matches!(source.as_str(), "auto" | "remote") {
            match remote_fresh_auth(ctx, &self.key).await {
                Ok(Some(auth)) if auth.is_fresh() => {
                    return self.fetch_billing(ctx, &auth, token_price).await
                }
                Ok(Some(_)) => {
                    let host = remote_host(ctx, &self.key).unwrap_or_default();
                    expired_hint = Some(format!(
                        "hermes token on {host} expired and auto-refresh failed — run `hermes auth status nous` there"
                    ));
                }
                Ok(None) if source == "remote" => {
                    return Err(FetchError::NotConfigured(
                        "set the remote SSH host (user@server) in Settings → Hermes".into(),
                    ))
                }
                Ok(None) => {}
                // A configured-but-unreachable remote is a real error worth
                // surfacing over falling back silently.
                Err(e) if source == "remote" => return Err(e),
                Err(e) => expired_hint = Some(e.to_string()),
            }
        }

        if source == "cookie"
            || ctx
                .secrets
                .get(&self.key)
                .filter(|c| !c.is_empty())
                .is_some()
        {
            return self.fetch_cookie(ctx, token_price).await;
        }
        match expired_hint {
            Some(hint) => Err(FetchError::AuthExpired(hint)),
            None => Err(FetchError::NotConfigured(
                "run hermes-agent on this machine, set a remote SSH host, or paste a portal session cookie in Settings"
                    .into(),
            )),
        }
    }
}

fn remote_host(ctx: &ProviderCtx, key: &str) -> Option<String> {
    ctx.config.provider_setting(key, "ssh_host").and_then(|v| {
        v.as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    })
}

/// The refresh command run (locally or over SSH) when the token is stale.
/// `hermes auth status nous` is lightweight, non-interactive, exits promptly,
/// and hermes itself performs the rotation-safe refresh + persistence. PATH is
/// widened because non-interactive SSH shells often lack ~/.local/bin.
/// Overridable via the `refresh_cmd` provider setting (e.g. Nix store paths).
const DEFAULT_REFRESH_CMD: &str =
    "PATH=\"$HOME/.local/bin:$HOME/bin:$PATH\" hermes auth status nous";
const REFRESH_TIMEOUT: Duration = std::time::Duration::from_secs(45);

use std::time::Duration;

fn refresh_cmd(ctx: &ProviderCtx, key: &str) -> String {
    ctx.config
        .provider_setting(key, "refresh_cmd")
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| DEFAULT_REFRESH_CMD.into())
}

/// Run a command on the remote host over SSH (BatchMode — the user's existing
/// keys/agent must authenticate; we never prompt).
async fn run_ssh(
    ctx: &ProviderCtx,
    key: &str,
    host: &str,
    remote_cmd: &str,
) -> Result<Vec<u8>, FetchError> {
    let program = ctx
        .config
        .provider_setting(key, "ssh_program")
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| "ssh".into());
    let mut cmd = tokio::process::Command::new(&program);
    cmd.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=5", host])
        .arg(remote_cmd)
        .stdin(std::process::Stdio::null());
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW — no console flash
    let out = tokio::time::timeout(REFRESH_TIMEOUT, cmd.output())
        .await
        .map_err(|_| FetchError::Network(format!("ssh {host}: timed out")))?
        .map_err(|e| {
            FetchError::Network(format!(
                "could not run `{program}`: {e} — is the OpenSSH client installed?"
            ))
        })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(FetchError::Network(format!(
            "ssh {host}: {}",
            stderr.lines().last().unwrap_or("failed").trim()
        )));
    }
    Ok(out.stdout)
}

fn remote_auth_path(ctx: &ProviderCtx, key: &str) -> String {
    ctx.config
        .provider_setting(key, "ssh_auth_path")
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_else(|| ".hermes/auth.json".into())
}

async fn read_remote(ctx: &ProviderCtx, key: &str, host: &str) -> Result<NousAuth, FetchError> {
    let path = remote_auth_path(ctx, key);
    let bytes = run_ssh(ctx, key, host, &format!("cat {path}")).await?;
    parse_hermes_auth(&String::from_utf8_lossy(&bytes))
        .ok_or_else(|| FetchError::Parse(format!("no Nous login in {host}:{path}")))
}

/// Fetch the hermes auth file from the remote machine; when it's stale, ask
/// the remote hermes CLI to refresh it and re-read once. Returns Ok(None)
/// when no remote host is configured. The returned auth may still be stale —
/// the caller checks `is_fresh()`.
async fn remote_fresh_auth(ctx: &ProviderCtx, key: &str) -> Result<Option<NousAuth>, FetchError> {
    let Some(host) = remote_host(ctx, key) else {
        return Ok(None);
    };
    let auth = read_remote(ctx, key, &host).await?;
    if auth.is_fresh() {
        return Ok(Some(auth));
    }
    if run_ssh(ctx, key, &host, &refresh_cmd(ctx, key))
        .await
        .is_ok()
    {
        return read_remote(ctx, key, &host).await.map(Some);
    }
    Ok(Some(auth))
}

/// Ask a locally-installed hermes CLI to refresh its login. Best-effort:
/// missing binary or failure just returns false.
async fn run_local_refresh(ctx: &ProviderCtx, key: &str) -> bool {
    let shell_cmd = refresh_cmd(ctx, key);
    #[cfg(windows)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", "hermes auth status nous"]);
        c.creation_flags(0x0800_0000);
        let _ = &shell_cmd; // PATH-widening form is POSIX-shell only
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", &shell_cmd]);
        c
    };
    cmd.stdin(std::process::Stdio::null());
    matches!(
        tokio::time::timeout(REFRESH_TIMEOUT, cmd.output()).await,
        Ok(Ok(out)) if out.status.success()
    )
}

pub struct NousAuth {
    pub access_token: String,
    pub portal_base: String,
    pub expires_at: Option<DateTime<Utc>>,
}

impl NousAuth {
    /// Fresh enough to use (30 s of clock-skew slack). Unknown expiry is
    /// treated as fresh — the API's 401 handles it.
    pub fn is_fresh(&self) -> bool {
        match self.expires_at {
            Some(exp) => exp >= Utc::now() + chrono::Duration::seconds(30),
            None => true,
        }
    }
}

/// Read the hermes-agent Nous login. Returns None when the file or token is
/// absent (not an error — other sources may still be configured).
pub fn read_hermes_auth(home: &Path) -> Option<NousAuth> {
    let text = std::fs::read_to_string(home.join(".hermes").join("auth.json")).ok()?;
    parse_hermes_auth(&text)
}

pub fn parse_hermes_auth(text: &str) -> Option<NousAuth> {
    let v: Value = serde_json::from_str(text).ok()?;
    let nous = &v["providers"]["nous"];
    let access = nous["access_token"].as_str()?.to_string();
    if access.is_empty() {
        return None;
    }
    Some(NousAuth {
        access_token: access,
        portal_base: nous["portal_base_url"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_PORTAL)
            .trim_end_matches('/')
            .to_string(),
        expires_at: nous["expires_at"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc)),
    })
}

impl Hermes {
    pub fn new(key: String, label: Option<String>) -> Self {
        Self { key, label }
    }
    async fn fetch_billing(
        &self,
        ctx: &ProviderCtx,
        auth: &NousAuth,
        token_price: Option<f64>,
    ) -> Result<UsageSnapshot, FetchError> {
        const STALE_HINT: &str =
            "hermes-agent token expired — run any `hermes` command (or keep hermes running) to refresh it";
        if let Some(exp) = auth.expires_at {
            if exp < Utc::now() + chrono::Duration::seconds(30) {
                return Err(FetchError::AuthExpired(STALE_HINT.into()));
            }
        }
        let url = format!("{}/api/billing/state", auth.portal_base);
        let resp = ctx
            .http
            .get(&url)
            .bearer_auth(&auth.access_token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(network_err)?;
        match resp.status().as_u16() {
            200..=299 => {}
            401 | 403 => return Err(FetchError::AuthExpired(STALE_HINT.into())),
            s => return Err(FetchError::Network(format!("HTTP {s} from {url}"))),
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        let (credits, mut windows) = parse_billing_state(&body, token_price)
            .ok_or_else(|| FetchError::Parse("billing state response missing balanceUsd".into()))?;

        // Subscription allowance (tier, monthly credits, cycle reset) — best
        // effort; a failure here still leaves a valid balance card.
        let sub_url = format!("{}/api/billing/subscription", auth.portal_base);
        if let Ok(resp) = ctx
            .http
            .get(&sub_url)
            .bearer_auth(&auth.access_token)
            .send()
            .await
        {
            if resp.status().is_success() {
                if let Ok(sub) = resp.json::<Value>().await {
                    windows.extend(parse_subscription(&sub, credits.balance));
                }
            }
        }
        Ok(UsageSnapshot::ok(
            self.id(),
            self.name(),
            windows,
            Some(credits),
        ))
    }

    async fn fetch_cookie(
        &self,
        ctx: &ProviderCtx,
        token_price: Option<f64>,
    ) -> Result<UsageSnapshot, FetchError> {
        let cookie = ctx
            .secrets
            .get(&self.key)
            .filter(|c| !c.is_empty())
            .ok_or_else(|| {
                FetchError::NotConfigured(
                    "run hermes-agent on this machine (its login is reused automatically) \
                 or paste a portal session cookie in Settings"
                        .into(),
                )
            })?;
        let endpoint = ctx
            .config
            .provider_setting(&self.key, "endpoint")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| format!("{DEFAULT_PORTAL}/api/billing/state"));

        let resp = ctx
            .http
            .get(&endpoint)
            .header("Cookie", cookie)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(network_err)?;
        match resp.status().as_u16() {
            200..=299 => {}
            401 | 403 => {
                return Err(FetchError::AuthExpired(
                    "session cookie expired — log in to the portal and re-paste it".into(),
                ))
            }
            s => return Err(FetchError::Network(format!("HTTP {s} from {endpoint}"))),
        }
        let body: Value = resp.json().await.map_err(network_err)?;
        let parsed = parse_billing_state(&body, token_price)
            .or_else(|| parse_lenient(&body, token_price))
            .ok_or_else(|| {
                FetchError::Parse(
                    "no balance-like field found in response — check the endpoint in Settings"
                        .into(),
                )
            })?;
        Ok(UsageSnapshot::ok(
            self.id(),
            self.name(),
            parsed.1,
            Some(parsed.0),
        ))
    }
}

/// Parse the portal's `/api/billing/state` shape (camelCase, money as strings):
/// `{"balanceUsd": "142.5", "monthlyCap": {"limitUsd": "1000", "spentThisMonthUsd": "180"}, …}`
fn parse_billing_state(
    body: &Value,
    token_price: Option<f64>,
) -> Option<(Credits, Vec<UsageWindow>)> {
    let balance = body.get("balanceUsd").and_then(as_f64)?;
    let mut windows = Vec::new();
    let mut used = None;
    if let Some(cap) = body.get("monthlyCap").filter(|c| c.is_object()) {
        let spent = cap.get("spentThisMonthUsd").and_then(as_f64);
        used = spent;
        if let (Some(limit), Some(spent)) = (cap.get("limitUsd").and_then(as_f64), spent) {
            if limit > 0.0 {
                windows.push(UsageWindow {
                    label: "Monthly cap".into(),
                    used_pct: spent / limit * 100.0,
                    resets_at: None,
                    ..Default::default()
                });
            }
        }
    }
    Some((make_credits(balance, used, token_price), windows))
}

/// Parse `/api/billing/subscription`:
/// `{"current": {"tierName": "Plus", "monthlyCredits": "22", "creditsRemaining": "3.5",
///   "cycleEndsAt": "2026-08-01T20:29:04.000Z", …}, …}`
///
/// `balance` is the purchased-credit balance, used to decide whether an
/// exhausted allowance is actually a problem: on the Free tier the portal
/// grants a fraction of a credit, so it reads 100% used forever while a funded
/// balance quietly pays for every call. Such a window is informational —
/// visible, but not driving the status colour, the tray, or alerts.
fn parse_subscription(body: &Value, balance: f64) -> Vec<UsageWindow> {
    let cur = &body["current"];
    let (Some(monthly), Some(remaining)) = (
        cur.get("monthlyCredits").and_then(as_f64),
        cur.get("creditsRemaining").and_then(as_f64),
    ) else {
        return vec![];
    };
    if monthly <= 0.0 {
        return vec![];
    }
    let tier = cur["tierName"].as_str().unwrap_or("subscription");
    // A funded balance keeps calls working regardless of the allowance, so
    // don't let an exhausted one paint everything red.
    let informational = balance > 0.0;
    vec![UsageWindow {
        label: format!("Monthly allowance ({tier})"),
        used_pct: ((monthly - remaining) / monthly * 100.0).clamp(0.0, 100.0),
        resets_at: cur["cycleEndsAt"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Utc)),
        informational,
    }]
}

fn make_credits(balance: f64, used: Option<f64>, token_price: Option<f64>) -> Credits {
    Credits {
        balance,
        unit: "USD".into(),
        used,
        granted: None,
        est_tokens_remaining: token_price.filter(|p| *p > 0.0).map(|p| balance / p),
    }
}

// ---- lenient fallback for unknown cookie-mode endpoints ---------------------

const BALANCE_KEYS: &[&str] = &[
    "balanceusd",
    "credits_remaining",
    "creditsremaining",
    "remaining_credits",
    "credit_balance",
    "balance",
    "credits",
];
const USED_KEYS: &[&str] = &[
    "credits_used",
    "creditsused",
    "used_credits",
    "usage",
    "used",
];

fn parse_lenient(body: &Value, token_price: Option<f64>) -> Option<(Credits, Vec<UsageWindow>)> {
    let balance = find_numeric(body, BALANCE_KEYS)?;
    let used = find_numeric(body, USED_KEYS);
    Some((make_credits(balance, used, token_price), vec![]))
}

fn find_numeric(v: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| find_by_key(v, key))
}

fn find_by_key(v: &Value, key: &str) -> Option<f64> {
    match v {
        Value::Object(obj) => {
            for (k, val) in obj {
                if k.to_ascii_lowercase() == key {
                    if let Some(n) = as_f64(val) {
                        return Some(n);
                    }
                }
            }
            obj.values().find_map(|val| find_by_key(val, key))
        }
        Value::Array(arr) => arr.iter().find_map(|val| find_by_key(val, key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shape from hermes-agent's own test fixtures for /api/billing/state.
    #[test]
    fn parses_portal_billing_state() {
        let body = serde_json::json!({
            "org": {"id": "o1", "slug": "acme", "name": "Acme", "role": "OWNER"},
            "balanceUsd": "142.5",
            "cliBillingEnabled": true,
            "monthlyCap": {"limitUsd": "1000", "spentThisMonthUsd": "180", "isDefaultCeiling": true},
            "autoReload": {"enabled": true, "thresholdUsd": "20"}
        });
        let (c, w) = parse_billing_state(&body, Some(0.000002)).unwrap();
        assert_eq!(c.balance, 142.5);
        assert_eq!(c.used, Some(180.0));
        assert!((c.est_tokens_remaining.unwrap() - 71_250_000.0).abs() < 1.0);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].label, "Monthly cap");
        assert!((w[0].used_pct - 18.0).abs() < 1e-9);
    }

    #[test]
    fn billing_state_without_cap_has_no_windows() {
        let body = serde_json::json!({"balanceUsd": "9.75", "monthlyCap": null});
        let (c, w) = parse_billing_state(&body, None).unwrap();
        assert_eq!(c.balance, 9.75);
        assert!(w.is_empty());
    }

    #[test]
    fn lenient_fallback_finds_nested_balance() {
        let body = serde_json::json!({
            "user": {"plan": "hermes-pro"},
            "billing": {"credits_remaining": 1234.5, "credits_used": 765.5}
        });
        let (c, _) = parse_lenient(&body, None).unwrap();
        assert_eq!(c.balance, 1234.5);
        assert_eq!(c.used, Some(765.5));
    }

    /// Shape captured live from the portal on 2026-07-31.
    #[test]
    fn parses_subscription_allowance() {
        let body = serde_json::json!({
            "current": {
                "tierName": "Free",
                "monthlyCredits": "0.1",
                "creditsRemaining": "0",
                "cycleEndsAt": "2026-08-01T20:29:04.000Z"
            }
        });
        let w = parse_subscription(&body, 0.0);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].label, "Monthly allowance (Free)");
        assert_eq!(w[0].used_pct, 100.0);
        assert!(w[0].resets_at.is_some());
        // zero-allowance tiers and missing fields produce no window
        assert!(parse_subscription(
            &serde_json::json!({"current": {"monthlyCredits": "0", "creditsRemaining": "0"}}),
            0.0
        )
        .is_empty());
        assert!(parse_subscription(&serde_json::json!({}), 0.0).is_empty());
    }

    /// The Free tier grants a fraction of a credit, so its allowance reads
    /// 100% used forever. With a purchased balance still paying for calls
    /// that isn't a quota problem — the window must not colour the card.
    #[test]
    fn exhausted_allowance_is_informational_when_balance_funds_calls() {
        let body = serde_json::json!({
            "current": {"tierName": "Free", "monthlyCredits": "0.1", "creditsRemaining": "0"}
        });

        let funded = parse_subscription(&body, 142.5);
        assert_eq!(funded[0].used_pct, 100.0);
        assert!(
            funded[0].informational,
            "funded balance should not report critical"
        );

        // No balance left: the exhausted allowance is a genuine block.
        let broke = parse_subscription(&body, 0.0);
        assert!(!broke[0].informational);

        let snap = UsageSnapshot::ok("hermes", "Hermes Portal", funded, None);
        assert_eq!(snap.status(80.0, 95.0, None), crate::model::Status::Ok);
        assert_eq!(
            UsageSnapshot::ok("hermes", "Hermes Portal", broke, None).status(80.0, 95.0, None),
            crate::model::Status::Critical
        );
    }

    fn stub_ctx(dir: &std::path::Path, script: &str) -> ProviderCtx {
        use crate::config::Config;
        let stub = dir.join("fake-ssh.sh");
        std::fs::write(&stub, script).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut cfg = Config::default();
        let settings = &mut cfg.providers.get_mut("hermes").unwrap().settings;
        settings.insert("ssh_host".into(), "ian@server".into());
        settings.insert(
            "ssh_program".into(),
            stub.to_string_lossy().into_owned().into(),
        );
        ProviderCtx::new(dir.into(), Default::default(), cfg)
    }

    const FRESH: &str = r#"{\"providers\":{\"nous\":{\"access_token\":\"remote-tok\",\"expires_at\":\"2099-01-01T00:00:00+00:00\"}}}"#;
    const STALE: &str = r#"{\"providers\":{\"nous\":{\"access_token\":\"old-tok\",\"expires_at\":\"2000-01-01T00:00:00+00:00\"}}}"#;

    #[tokio::test]
    async fn remote_auth_via_stub_ssh() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = stub_ctx(dir.path(), &format!("#!/bin/sh\necho \"{FRESH}\"\n"));
        let auth = remote_fresh_auth(&ctx, "hermes").await.unwrap().unwrap();
        assert_eq!(auth.access_token, "remote-tok");
        assert!(auth.is_fresh());
    }

    /// Stale token → the adapter runs the refresh command on the "remote"
    /// (stub records it), then re-reads and gets the fresh token.
    #[tokio::test]
    async fn remote_auth_stale_triggers_refresh_and_reread() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("refreshed");
        // $6 is the remote command: `cat .hermes/auth.json` or the refresh cmd.
        let script = format!(
            "#!/bin/sh\ncase \"$6\" in\n  'cat .hermes/auth.json')\n    if [ -f {m} ]; then echo \"{FRESH}\"; else echo \"{STALE}\"; fi ;;\n  *) touch {m} ;;\nesac\n",
            m = marker.display()
        );
        let ctx = stub_ctx(dir.path(), &script);
        let auth = remote_fresh_auth(&ctx, "hermes").await.unwrap().unwrap();
        assert!(marker.exists(), "refresh command was not invoked");
        assert_eq!(auth.access_token, "remote-tok");
        assert!(auth.is_fresh());
    }

    #[tokio::test]
    async fn remote_auth_unconfigured_is_none_and_failure_is_network() {
        use crate::config::Config;
        let ctx = ProviderCtx::new(std::env::temp_dir(), Default::default(), Config::default());
        assert!(remote_fresh_auth(&ctx, "hermes").await.unwrap().is_none());

        let mut cfg = Config::default();
        let settings = &mut cfg.providers.get_mut("hermes").unwrap().settings;
        settings.insert("ssh_host".into(), "ian@server".into());
        settings.insert("ssh_program".into(), "/nonexistent/ssh".into());
        let ctx = ProviderCtx::new(std::env::temp_dir(), Default::default(), cfg);
        assert!(matches!(
            remote_fresh_auth(&ctx, "hermes").await,
            Err(FetchError::Network(_))
        ));
    }

    #[test]
    fn reads_hermes_auth_json() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".hermes")).unwrap();
        std::fs::write(
            dir.path().join(".hermes/auth.json"),
            serde_json::json!({
                "providers": {"nous": {
                    "access_token": "tok",
                    "portal_base_url": "https://portal.nousresearch.com/",
                    "expires_at": "2026-07-31T14:30:17+00:00"
                }}
            })
            .to_string(),
        )
        .unwrap();
        let auth = read_hermes_auth(dir.path()).unwrap();
        assert_eq!(auth.access_token, "tok");
        assert_eq!(auth.portal_base, "https://portal.nousresearch.com");
        assert!(auth.expires_at.is_some());
        // absent file → None
        assert!(read_hermes_auth(Path::new("/nonexistent")).is_none());
    }
}
