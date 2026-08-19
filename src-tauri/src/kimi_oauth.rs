//! Built-in Kimi Code sign-in via the device flow in Moonshot's published Kimi
//! Code CLI. We use its public client id but do not claim to be that CLI in
//! headers: quota-widget has its own product identity.
//!
//! 1. POST `https://auth.kimi.com/api/oauth/device_authorization` with the
//!    client id.
//! 2. The user opens `verification_uri_complete` and approves the code.
//! 3. Poll `/api/oauth/token` with the device-code grant until a token arrives.

use quota_core::providers::moonshot::KIMI_CLIENT_ID;
use serde_json::Value;
use std::time::Duration;

const OAUTH_HOST: &str = "https://auth.kimi.com";
const MIN_WAIT: Duration = Duration::from_secs(10 * 60);
const DEFAULT_POLL: Duration = Duration::from_secs(5);
const SLOW_DOWN_INCREMENT: Duration = Duration::from_secs(5);

pub struct DeviceLogin {
    pub user_code: String,
    pub verification_url: String,
    device_code: String,
    interval: Duration,
    expires_in: i64,
}

pub async fn start(http: &reqwest::Client) -> Result<DeviceLogin, String> {
    let resp = http
        .post(format!("{OAUTH_HOST}/api/oauth/device_authorization"))
        .header("Accept", "application/json")
        .form(&[("client_id", KIMI_CLIENT_ID)])
        .send()
        .await
        .map_err(|e| format!("could not reach Kimi Code sign-in: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("sign-in request rejected (HTTP {})", resp.status()));
    }
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("unexpected Kimi Code sign-in response: {e}"))?;
    Ok(DeviceLogin {
        device_code: required(&body, "device_code")?,
        user_code: required(&body, "user_code")?,
        verification_url: body["verification_uri_complete"]
            .as_str()
            .or_else(|| body["verification_uri"].as_str())
            .filter(|url| !url.is_empty())
            .ok_or("sign-in response missing verification URI")?
            .to_string(),
        interval: body["interval"]
            .as_u64()
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_POLL),
        expires_in: body["expires_in"].as_i64().unwrap_or(0),
    })
}

pub async fn poll_for_tokens(http: &reqwest::Client, login: &DeviceLogin) -> Result<Value, String> {
    let mut interval = login.interval.max(Duration::from_secs(1));
    let deadline = std::time::Instant::now()
        + Duration::from_secs(login.expires_in.max(0) as u64).max(MIN_WAIT);
    loop {
        tokio::time::sleep(interval).await;
        if std::time::Instant::now() >= deadline {
            return Err("sign-in timed out — start again".into());
        }
        let resp = http
            .post(format!("{OAUTH_HOST}/api/oauth/token"))
            .header("Accept", "application/json")
            .form(&[
                ("client_id", KIMI_CLIENT_ID),
                ("device_code", login.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(|e| format!("sign-in poll failed: {e}"))?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| format!("unexpected Kimi Code token response: {e}"))?;
        if status.is_success() {
            return token_secret(&body);
        }
        match body["error"].as_str().unwrap_or("") {
            "authorization_pending" => continue,
            "slow_down" => {
                interval += SLOW_DOWN_INCREMENT;
                continue;
            }
            "access_denied" => return Err("sign-in was denied in the browser".into()),
            "expired_token" => return Err("the sign-in code expired — start again".into()),
            error => {
                return Err(format!(
                    "sign-in failed: {}",
                    body["error_description"].as_str().unwrap_or(error)
                ))
            }
        }
    }
}

fn required(body: &Value, field: &str) -> Result<String, String> {
    body[field]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("sign-in response missing {field}"))
}

fn token_secret(body: &Value) -> Result<Value, String> {
    let access = required(body, "access_token")?;
    let refresh = required(body, "refresh_token")?;
    let expires_in = body["expires_in"]
        .as_i64()
        .filter(|value| *value > 0)
        .ok_or("token response missing expires_in")?;
    Ok(serde_json::json!({
        "access_token": access,
        "refresh_token": refresh,
        "expires_at": chrono::Utc::now().timestamp() + expires_in,
        "expires_in": expires_in,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_the_core_adapter_token_shape() {
        let secret = token_secret(&serde_json::json!({"access_token": "access", "refresh_token": "refresh", "expires_in": 3600})).unwrap();
        assert_eq!(secret["access_token"], "access");
        assert_eq!(secret["refresh_token"], "refresh");
        assert!(secret["expires_at"].as_i64().unwrap() > 0);
    }
}
