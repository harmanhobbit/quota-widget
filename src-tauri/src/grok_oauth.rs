//! Built-in Grok (SuperGrok) sign-in via the RFC 8628 device flow the grok
//! CLI's `grok login --device-auth` uses.
//!
//! Protocol read from `github.com/xai-org/grok-build`
//! (`xai-grok-shell/src/auth/device_code.rs`, `auth/config.rs`); the endpoints
//! are CLI-internal and undocumented, so these constants are pinned to that
//! clone:
//!
//! 1. POST `{issuer}/oauth2/device/code` (form `client_id`, `scope`,
//!    `referrer`) -> `device_code`, `user_code`, `verification_uri(_complete)`,
//!    `interval`, `expires_in`.
//! 2. User opens the verification URL and approves the `user_code`.
//! 3. Poll POST `{issuer}/oauth2/token` (form
//!    `grant_type=urn:ietf:params:oauth:grant-type:device_code`, `device_code`,
//!    `client_id`) until it stops returning `authorization_pending` ->
//!    `access_token` / `refresh_token` / `expires_in` / `id_token`.
//!
//! Polling is error-code-driven per RFC 8628: `authorization_pending` and
//! `slow_down` keep waiting (the latter backing off), anything else is fatal.
//!
//! The persisted secret is the widget's own `grok_oauth` shape
//! (`accessToken`/`refreshToken`/`expiresAt`(ms)/`userId`) that
//! `quota_core::providers::grok` parses — never the CLI's `auth.json`.

use base64::Engine;
use serde_json::Value;
use std::time::Duration;

// The grok CLI's public OAuth client id (shared with the core adapter).
use quota_core::providers::grok::CLIENT_ID;

const ISSUER: &str = "https://auth.x.ai";
/// `x-grok-client-version` pinned to the grok-build clone (see the adapter).
const CLIENT_VERSION: &str = "1.0.4";
/// Usage-attribution referrer the CLI sends on the device-code request.
const REFERRER: &str = "grok-build";
/// The 10 frozen scopes the xAI OAuth2 client requests.
const SCOPES: &str = "openid profile email offline_access grok-cli:access api:access conversations:read conversations:write workspaces:read workspaces:write";
/// The device code expires server-side; floor the poll deadline so a missing
/// `expires_in` still bounds the wait.
const MIN_WAIT: Duration = Duration::from_secs(10 * 60);
/// RFC 8628 default when the server omits `interval`.
const DEFAULT_POLL: Duration = Duration::from_secs(5);
const SLOW_DOWN_INCREMENT: Duration = Duration::from_secs(5);

/// Handed to the UI so the user can complete sign-in in a browser.
pub struct DeviceLogin {
    pub user_code: String,
    pub verification_url: String,
    device_code: String,
    interval: Duration,
    expires_in: i64,
}

/// Step 1: request a device + user code.
pub async fn start(http: &reqwest::Client) -> Result<DeviceLogin, String> {
    let resp = http
        .post(format!("{ISSUER}/oauth2/device/code"))
        .header("x-grok-client-version", CLIENT_VERSION)
        .form(&[
            ("client_id", CLIENT_ID),
            ("scope", SCOPES),
            ("referrer", REFERRER),
        ])
        .send()
        .await
        .map_err(|e| format!("could not reach xAI sign-in: {e}"))?;

    if resp.status().as_u16() == 404 {
        return Err(
            "device sign-in isn't available for this account — run `grok login` instead".into(),
        );
    }
    if !resp.status().is_success() {
        return Err(format!("sign-in request rejected (HTTP {})", resp.status()));
    }

    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("unexpected sign-in response: {e}"))?;
    let device_code = body["device_code"]
        .as_str()
        .ok_or("sign-in response missing device_code")?
        .to_string();
    let user_code = body["user_code"]
        .as_str()
        .ok_or("sign-in response missing user_code")?
        .to_string();
    // Prefer the pre-filled URL (embeds the code); fall back to the bare one.
    let verification_url = body["verification_uri_complete"]
        .as_str()
        .or_else(|| body["verification_uri"].as_str())
        .ok_or("sign-in response missing verification_uri")?
        .to_string();

    Ok(DeviceLogin {
        user_code,
        verification_url,
        device_code,
        interval: body["interval"]
            .as_u64()
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_POLL),
        expires_in: body["expires_in"].as_i64().unwrap_or(0),
    })
}

/// Steps 3: poll until the user approves, then return the `grok_oauth` secret
/// JSON to persist.
pub async fn poll_for_tokens(http: &reqwest::Client, login: &DeviceLogin) -> Result<Value, String> {
    let mut interval = login.interval.max(Duration::from_secs(1));
    let wait = Duration::from_secs(login.expires_in.max(0) as u64).max(MIN_WAIT);
    let deadline = std::time::Instant::now() + wait;

    loop {
        // Sleep first: an immediate poll on a fresh code only returns
        // authorization_pending (and risks slow_down).
        tokio::time::sleep(interval).await;
        if std::time::Instant::now() >= deadline {
            return Err("sign-in timed out — start again".into());
        }

        let resp = http
            .post(format!("{ISSUER}/oauth2/token"))
            .header("x-grok-client-version", CLIENT_VERSION)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", login.device_code.as_str()),
                ("client_id", CLIENT_ID),
            ])
            .send()
            .await
            .map_err(|e| format!("sign-in poll failed: {e}"))?;

        if resp.status().is_success() {
            let body: Value = resp
                .json()
                .await
                .map_err(|e| format!("unexpected token response: {e}"))?;
            return Ok(secret_from_tokens(&body));
        }

        // Non-2xx carries an OAuth error code.
        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("unexpected token response: {e}"))?;
        match body["error"].as_str().unwrap_or("") {
            "authorization_pending" => continue,
            "slow_down" => {
                interval += SLOW_DOWN_INCREMENT;
                continue;
            }
            "access_denied" => return Err("sign-in was denied in the browser".into()),
            "expired_token" => return Err("the sign-in code expired — start again".into()),
            other => {
                let detail = body["error_description"].as_str().unwrap_or(other);
                return Err(format!("sign-in failed: {detail}"));
            }
        }
    }
}

/// Build the `grok_oauth` secret from a successful token response. `user_id`
/// comes from the id_token's `sub` claim; resolving it here means the adapter
/// never has to decode a JWT (and no id_token is stored).
fn secret_from_tokens(body: &Value) -> Value {
    let expires_at_ms = body["expires_in"]
        .as_i64()
        .map(|s| chrono::Utc::now().timestamp_millis() + s * 1000)
        .unwrap_or(0);
    let user_id = body["id_token"].as_str().and_then(sub_from_jwt);
    serde_json::json!({
        "accessToken": body["access_token"].as_str(),
        "refreshToken": body["refresh_token"].as_str(),
        "expiresAt": expires_at_ms,
        "userId": user_id,
    })
}

/// Decode a JWT's `sub` claim without verifying the signature — the token
/// arrives over a direct HTTPS channel (no browser redirect) and is used only
/// for the `x-userid` billing header.
fn sub_from_jwt(id_token: &str) -> Option<String> {
    let payload_b64 = id_token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims["sub"].as_str().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sub_extracted_from_id_token() {
        let claims = serde_json::json!({"sub": "user-123", "email": "a@b.test"});
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
        let jwt = format!("eyJhbGciOiJub25lIn0.{payload}.sig");
        assert_eq!(sub_from_jwt(&jwt).as_deref(), Some("user-123"));
    }

    #[test]
    fn secret_shape_carries_tokens_and_user() {
        let body = serde_json::json!({
            "access_token": "acc",
            "refresh_token": "ref",
            "expires_in": 3600,
            "id_token": {
                // A real id_token is a JWT string; here just exercise the None path.
            }
        });
        let secret = secret_from_tokens(&body);
        assert_eq!(secret["accessToken"], "acc");
        assert_eq!(secret["refreshToken"], "ref");
        assert!(secret["expiresAt"].as_i64().unwrap() > 0);
        assert!(secret["userId"].is_null());
    }
}
