//! Built-in Claude sign-in: the same PKCE code flow Claude Code uses, with the
//! manual paste-back variant (`code=true`) so no local callback server is
//! needed. The browser lands on an Anthropic page showing `code#state`, which
//! the user pastes into Settings.

use base64::Engine;
use quota_core::providers::claude::{TokenSet, CLIENT_ID};
use rand::RngCore;
use sha2::{Digest, Sha256};

const AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const SCOPES: &str = "org:create_api_key user:profile user:inference";

pub struct PendingLogin {
    pub verifier: String,
    pub state: String,
}

fn random_b64url(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// Build the authorize URL and the pending state to verify the paste-back.
pub fn start() -> (String, PendingLogin) {
    let verifier = random_b64url(48);
    let challenge =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_b64url(24);
    let url = format!(
        "{AUTHORIZE_URL}?code=true&client_id={CLIENT_ID}&response_type=code\
         &redirect_uri={}&scope={}&code_challenge={challenge}&code_challenge_method=S256&state={state}",
        urlencode(REDIRECT_URI),
        urlencode(SCOPES),
    );
    (url, PendingLogin { verifier, state })
}

/// Exchange the pasted `code#state` string for tokens.
pub async fn finish(
    http: &reqwest::Client,
    pending: &PendingLogin,
    pasted: &str,
) -> Result<TokenSet, String> {
    let pasted = pasted.trim();
    let (code, state) = match pasted.split_once('#') {
        Some((c, s)) => (c.trim(), s.trim()),
        None => (pasted, pending.state.as_str()),
    };
    if code.is_empty() {
        return Err("empty code — paste the full string shown after authorizing".into());
    }
    if state != pending.state {
        return Err("state mismatch — restart the sign-in and paste the newest code".into());
    }
    let resp = http
        .post(TOKEN_URL)
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "state": state,
            "client_id": CLIENT_ID,
            "redirect_uri": REDIRECT_URI,
            "code_verifier": pending.verifier,
        }))
        .send()
        .await
        .map_err(|e| format!("token exchange failed: {e}"))?;
    let status = resp.status();
    let body: serde_json::Value =
        resp.json().await.map_err(|e| format!("token exchange failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("token exchange rejected (HTTP {status}): {body}"));
    }
    let access = body["access_token"]
        .as_str()
        .ok_or_else(|| "token response missing access_token".to_string())?
        .to_string();
    let expires_at_ms = body["expires_in"]
        .as_i64()
        .map(|s| chrono::Utc::now().timestamp_millis() + s * 1000)
        .unwrap_or(0);
    Ok(TokenSet {
        access,
        refresh: body["refresh_token"].as_str().map(String::from),
        expires_at_ms,
    })
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
