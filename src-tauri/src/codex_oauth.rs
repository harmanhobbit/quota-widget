//! Built-in Codex sign-in via the device flow the Codex CLI's
//! `codex login --device-auth` uses.
//!
//! This is *not* RFC 8628 despite the name, so the usual device-flow
//! assumptions do not apply. Protocol read from openai/codex @ ee0247f9
//! (`codex-rs/login/src/device_code_auth.rs`, `server.rs`); it is undocumented,
//! so treat these constants as pinned to that commit:
//!
//! 1. POST `/api/accounts/deviceauth/usercode` (JSON, client_id only)
//!    -> `device_auth_id`, `user_code`, `interval`
//! 2. User opens `{issuer}/codex/device` and types the `user_code`.
//! 3. Poll POST `/api/accounts/deviceauth/token` (JSON) until it stops
//!    returning 403/404 -> `authorization_code` **and a server-generated
//!    `code_verifier`** (we never compute a PKCE challenge ourselves).
//! 4. POST `/oauth/token` (form-encoded) with `grant_type=authorization_code`
//!    and that verifier -> `id_token` / `access_token` / `refresh_token`.
//!
//! Polling is status-driven, not error-code-driven: 403/404 mean "pending",
//! anything else non-2xx is fatal. There is no `slow_down` back-off.

use serde_json::Value;
use std::time::Duration;

const ISSUER: &str = "https://auth.openai.com";
/// The Codex CLI's public client id — shared by both its login flows.
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// Constructed client-side and never navigated to; the exchange still
/// requires it to match.
const REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
/// The CLI gives the user 15 minutes; the server-side code expires with it.
const MAX_WAIT: Duration = Duration::from_secs(15 * 60);
/// `interval` is absent or "0" in practice; don't hammer the endpoint.
const MIN_POLL: Duration = Duration::from_secs(2);

/// Handed to the UI so the user can complete sign-in in a browser.
pub struct DeviceLogin {
    pub device_auth_id: String,
    pub user_code: String,
    pub verification_url: String,
    pub interval: Duration,
}

/// Step 1: ask for a user code.
pub async fn start(http: &reqwest::Client) -> Result<DeviceLogin, String> {
    let resp = http
        .post(format!("{ISSUER}/api/accounts/deviceauth/usercode"))
        .json(&serde_json::json!({ "client_id": CLIENT_ID }))
        .send()
        .await
        .map_err(|e| format!("could not reach OpenAI sign-in: {e}"))?;

    // The CLI special-cases 404 here: it means device login isn't enabled for
    // the account/workspace, not that the endpoint is missing.
    if resp.status().as_u16() == 404 {
        return Err("device sign-in isn't enabled for this account — an admin may need to turn it on, or run `codex login` instead".into());
    }
    if !resp.status().is_success() {
        return Err(format!("sign-in request rejected (HTTP {})", resp.status()));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("unexpected sign-in response: {e}"))?;
    let device_auth_id = body["device_auth_id"]
        .as_str()
        .ok_or("sign-in response missing device_auth_id")?
        .to_string();
    // Aliased `user_code` / `usercode` upstream.
    let user_code = body["user_code"]
        .as_str()
        .or_else(|| body["usercode"].as_str())
        .ok_or("sign-in response missing user_code")?
        .to_string();

    Ok(DeviceLogin {
        device_auth_id,
        user_code,
        verification_url: format!("{ISSUER}/codex/device"),
        // Sent as a *string* upstream, and may be absent entirely.
        interval: interval_from(&body["interval"]).max(MIN_POLL),
    })
}

fn interval_from(v: &Value) -> Duration {
    let secs = v
        .as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0);
    Duration::from_secs(secs)
}

/// Steps 3–4: block until the user authorizes, then exchange for tokens.
/// Returns the `tokens` object to persist, in the CLI's own shape.
pub async fn poll_for_tokens(http: &reqwest::Client, login: &DeviceLogin) -> Result<Value, String> {
    let deadline = std::time::Instant::now() + MAX_WAIT;
    loop {
        if std::time::Instant::now() >= deadline {
            return Err("sign-in timed out after 15 minutes — start again".into());
        }
        let resp = http
            .post(format!("{ISSUER}/api/accounts/deviceauth/token"))
            .json(&serde_json::json!({
                "device_auth_id": login.device_auth_id,
                "user_code": login.user_code,
            }))
            .send()
            .await
            .map_err(|e| format!("sign-in poll failed: {e}"))?;

        match resp.status().as_u16() {
            // Not authorized yet — the only "keep waiting" signal there is.
            403 | 404 => {
                tokio::time::sleep(login.interval).await;
                continue;
            }
            200..=299 => {
                let body: Value = resp
                    .json()
                    .await
                    .map_err(|e| format!("unexpected poll response: {e}"))?;
                let code = body["authorization_code"]
                    .as_str()
                    .ok_or("poll response missing authorization_code")?;
                // PKCE inverted: the *server* generates the verifier and we
                // replay it. Nothing to compute or compare here.
                let verifier = body["code_verifier"]
                    .as_str()
                    .ok_or("poll response missing code_verifier")?;
                return exchange(http, code, verifier).await;
            }
            s => return Err(format!("sign-in failed (HTTP {s})")),
        }
    }
}

/// Step 4: the one form-encoded call in the flow.
async fn exchange(http: &reqwest::Client, code: &str, verifier: &str) -> Result<Value, String> {
    let resp = http
        .post(format!("{ISSUER}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .map_err(|e| format!("token exchange failed: {e}"))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("token exchange failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("token exchange rejected (HTTP {status})"));
    }
    let access = body["access_token"]
        .as_str()
        .ok_or("token response missing access_token")?;
    let id_token = body["id_token"].as_str().unwrap_or_default();

    // Store in the CLI's `tokens` shape so the provider adapter can read
    // either source with one parser. account_id isn't returned by any
    // endpoint — it comes out of the id_token's claims.
    Ok(serde_json::json!({
        "tokens": {
            "access_token": access,
            "refresh_token": body["refresh_token"].as_str(),
            "id_token": id_token,
            "account_id": quota_core::providers::codex::account_id_from_jwt(id_token),
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_accepts_string_number_and_missing() {
        assert_eq!(
            interval_from(&serde_json::json!("5")),
            Duration::from_secs(5)
        );
        assert_eq!(interval_from(&serde_json::json!(3)), Duration::from_secs(3));
        // Absent or unparseable falls back to 0, which the caller floors.
        assert_eq!(interval_from(&Value::Null), Duration::from_secs(0));
        assert_eq!(
            interval_from(&serde_json::json!("")),
            Duration::from_secs(0)
        );
    }
}
