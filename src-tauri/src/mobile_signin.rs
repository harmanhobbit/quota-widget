//! Built-in sign-in state for Android (Claude PKCE paste-back, Codex device
//! flow) plus encrypted persistence of in-flight attempts so process/Activity
//! death does not lose PKCE verifiers or device codes before expiry.
//!
//! The actual OAuth protocol lives in `oauth.rs` and `codex_oauth.rs`, which
//! are platform-agnostic; this module only owns the Android-specific concerns:
//! - launching the browser via the shared opener plugin,
//! - keeping one pending sign-in per provider in encrypted app storage,
//! - expiring stale pending records and requiring an explicit restart,
//! - surfacing a storage failure during token persistence as an error rather
//!   than a silent success.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Secret key used for the encrypted pending-sign-in blob. One blob holds all
/// pending attempts, which keeps the Keystore-backed secret surface small and
/// lets expiry cleanup atomically drop everything expired.
const PENDING_SIGNINS_SECRET_KEY: &str = "pending_signins";

/// One in-flight sign-in attempt. `kind` carries the protocol-specific state
/// needed to complete it; `expires_at` is set generously above each provider's
/// server-side code lifetime so the server rejects expired codes before we do.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingSignIn {
    pub provider_key: String,
    pub kind: PendingKind,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PendingKind {
    /// Claude PKCE paste-back: the verifier is the private PKCE secret, the
    /// state is the CSRF nonce we expect back from the authorization response.
    ClaudePkce { verifier: String, state: String },
    /// Codex device flow: the user types `user_code` at `verification_url`
    /// while we poll `device_auth_id` for completion.
    CodexDevice {
        device_auth_id: String,
        user_code: String,
        verification_url: String,
        interval_secs: u64,
    },
}

/// Serializable snapshot returned to the mobile UI so it can resume a pending
/// sign-in after process death. `kind` is intentionally a flat string so the
/// Svelte side can switch on it without importing Rust enum names.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingSignInView {
    pub provider_key: String,
    pub kind: String,
    pub user_code: Option<String>,
    pub verification_url: Option<String>,
    pub expires_at: DateTime<Utc>,
}

impl From<&PendingSignIn> for PendingSignInView {
    fn from(p: &PendingSignIn) -> Self {
        let mut view = Self {
            provider_key: p.provider_key.clone(),
            kind: match &p.kind {
                PendingKind::ClaudePkce { .. } => "claude_pkce".into(),
                PendingKind::CodexDevice { .. } => "codex_device".into(),
            },
            user_code: None,
            verification_url: None,
            expires_at: p.expires_at,
        };
        if let PendingKind::CodexDevice {
            user_code,
            verification_url,
            ..
        } = &p.kind
        {
            view.user_code = Some(user_code.clone());
            view.verification_url = Some(verification_url.clone());
        }
        view
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct PendingSignIns {
    pending: Vec<PendingSignIn>,
}

impl PendingSignIns {
    /// Strict load. A secure-storage failure surfaces as `Err` instead of being
    /// folded into "no pending sign-ins" — the whole point of issue #133 for the
    /// OAuth pending-state path: the generic `secrets::get` returns `None` for
    /// both "nothing stored" and "Keystore couldn't decrypt", so the completion
    /// and listing paths use `get_reporting_errors` to keep the distinction.
    /// Reading the pending blob when it can't be decrypted must not report a
    /// broken store as empty, nor let `finish`/`poll` proceed on a verifier they
    /// never actually recovered.
    fn load(dir: &Path) -> Result<Self, String> {
        Self::parse(crate::secrets::get_reporting_errors(
            dir,
            PENDING_SIGNINS_SECRET_KEY,
        ))
    }

    /// Interpret one backend read of the pending blob. Kept pure so the
    /// storage-error-vs-absent behaviour is unit-testable without a real
    /// Keystore.
    fn parse(read: Result<Option<String>, String>) -> Result<Self, String> {
        match read {
            Ok(Some(raw)) => serde_json::from_str(&raw).map_err(|e| e.to_string()),
            Ok(None) => Ok(Self::default()),
            Err(e) => Err(format!("pending sign-in storage unavailable: {e}")),
        }
    }

    /// Load for a path that is about to fully rewrite the blob (starting a new
    /// sign-in). An existing blob left undecryptable by a Keystore rotation must
    /// not permanently block starting a fresh sign-in — otherwise an OAuth
    /// account could never be recovered. Starting overwrites that provider's
    /// pending entry anyway, and the subsequent `save` is an explicit `set`
    /// (the only thing besides `clear` allowed to touch the ciphertext), so on
    /// an unreadable blob we begin from empty. A *write* failure still surfaces
    /// from `save`.
    fn load_for_replace(dir: &Path) -> Self {
        Self::load(dir).unwrap_or_default()
    }

    fn save(&self, dir: &Path) -> Result<(), String> {
        if self.pending.is_empty() {
            crate::secrets::clear(dir, PENDING_SIGNINS_SECRET_KEY)
        } else {
            crate::secrets::set(
                dir,
                PENDING_SIGNINS_SECRET_KEY,
                &serde_json::to_string(self).map_err(|e| e.to_string())?,
            )
        }
    }

    /// Removes every expired pending sign-in. Returns the number removed.
    fn cleanup_expired(&mut self) -> usize {
        let now = Utc::now();
        let before = self.pending.len();
        self.pending.retain(|p| p.expires_at > now);
        before - self.pending.len()
    }

    fn get(&self, provider_key: &str) -> Option<&PendingSignIn> {
        self.pending.iter().find(|p| p.provider_key == provider_key)
    }

    fn remove(&mut self, provider_key: &str) {
        self.pending.retain(|p| p.provider_key != provider_key);
    }

    fn insert(&mut self, item: PendingSignIn) {
        self.remove(&item.provider_key);
        self.pending.push(item);
    }
}

/// Builds an HTTP client with the same conservative timeout the shared
/// `ProviderCtx` uses. Sign-in exchanges are short, single-shot requests.
fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("quota-widget/0.1")
        .build()
        .map_err(|e| e.to_string())
}

/// Start a Claude PKCE sign-in. Returns the authorization URL to open in a
/// browser; the matching PKCE verifier/state are encrypted and stored pending.
pub async fn start_claude(dir: &Path, provider_key: &str) -> Result<String, String> {
    let (url, pending) = crate::oauth::start();
    let mut signins = PendingSignIns::load_for_replace(dir);
    signins.cleanup_expired();
    signins.insert(PendingSignIn {
        provider_key: provider_key.to_string(),
        kind: PendingKind::ClaudePkce {
            verifier: pending.verifier,
            state: pending.state,
        },
        // Claude's authorization code expires server-side after 10 minutes;
        // 15 minutes of local pending storage gives a little UI slack while
        // still guaranteeing an expired code is eventually discarded.
        expires_at: Utc::now() + Duration::minutes(15),
    });
    signins.save(dir)?;
    Ok(url)
}

/// Complete a Claude PKCE sign-in from the `code#state` string shown by the
/// browser after authorization. The pending state must still be present and
/// unexpired. Stores the resulting tokens under `{provider_key}_oauth`.
pub async fn finish_claude(dir: &Path, provider_key: &str, pasted: &str) -> Result<(), String> {
    let http = http_client()?;
    let mut signins = PendingSignIns::load(dir)?;
    signins.cleanup_expired();
    let pending = signins
        .get(provider_key)
        .ok_or("no pending Claude sign-in — start again")?;
    let PendingKind::ClaudePkce { verifier, state } = &pending.kind else {
        return Err("pending sign-in is not a Claude PKCE flow — start again".into());
    };
    let login = crate::oauth::PendingLogin {
        verifier: verifier.clone(),
        state: state.clone(),
    };
    let tokens = crate::oauth::finish(&http, &login, pasted).await?;
    crate::secrets::set(
        dir,
        &format!("{}_oauth", provider_key),
        &tokens.to_secret_json(),
    )?;
    signins.remove(provider_key);
    signins.save(dir)?;
    Ok(())
}

/// Information the Codex UI needs to display while waiting for the user to
/// authorize the device code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexLoginInfo {
    pub user_code: String,
    pub verification_url: String,
}

/// Start a Codex device-flow sign-in. Returns the user-facing code and URL;
/// the device auth id is encrypted and stored pending.
pub async fn start_codex(dir: &Path, provider_key: &str) -> Result<CodexLoginInfo, String> {
    let http = http_client()?;
    let login = crate::codex_oauth::start(&http).await?;
    let info = CodexLoginInfo {
        user_code: login.user_code.clone(),
        verification_url: login.verification_url.clone(),
    };
    let mut signins = PendingSignIns::load_for_replace(dir);
    signins.cleanup_expired();
    signins.insert(PendingSignIn {
        provider_key: provider_key.to_string(),
        kind: PendingKind::CodexDevice {
            device_auth_id: login.device_auth_id,
            user_code: login.user_code,
            verification_url: login.verification_url,
            interval_secs: login.interval.as_secs().max(2),
        },
        // OpenAI's device code lifetime is 15 minutes; keep local pending state
        // aligned so an abandoned flow is cleaned up without manual cancel.
        expires_at: Utc::now() + Duration::minutes(15),
    });
    signins.save(dir)?;
    Ok(info)
}

/// Result of one Codex device-flow poll attempt. Returned as a plain string
/// (`"complete"` or `"pending"`) so the Svelte side can switch on it directly.
pub type CodexPollResult = String;

/// Poll once for Codex device-flow completion. Returns `"complete"` only after
/// tokens have been successfully stored; a storage failure is returned as an
/// error so the caller can retry rather than believing the sign-in succeeded.
/// Returns `"pending"` when the user has not yet authorized the code.
pub async fn poll_codex(dir: &Path, provider_key: &str) -> Result<CodexPollResult, String> {
    let http = http_client()?;
    let mut signins = PendingSignIns::load(dir)?;
    signins.cleanup_expired();
    let pending = signins
        .get(provider_key)
        .ok_or("no pending Codex sign-in — start again")?;
    let PendingKind::CodexDevice {
        device_auth_id,
        user_code,
        verification_url,
        interval_secs,
    } = pending.kind.clone()
    else {
        return Err("pending sign-in is not a Codex device flow — start again".into());
    };
    let login = crate::codex_oauth::DeviceLogin {
        device_auth_id,
        user_code,
        verification_url,
        interval: std::time::Duration::from_secs(interval_secs),
    };
    match crate::codex_oauth::poll_once(&http, &login).await? {
        crate::codex_oauth::PollOutcome::Pending => Ok("pending".to_string()),
        crate::codex_oauth::PollOutcome::Complete(tokens) => {
            crate::secrets::set(dir, &format!("{}_oauth", provider_key), &tokens.to_string())?;
            signins.remove(provider_key);
            signins.save(dir)?;
            Ok("complete".to_string())
        }
    }
}

/// Cancel and drop any pending sign-in for the given provider. Must succeed
/// even when the blob can't be decrypted (a Keystore rotation), so the user is
/// never stuck unable to clear a broken pending state: if the read fails, clear
/// the whole pending key outright — an explicit removal, the recovery path.
pub fn cancel(dir: &Path, provider_key: &str) -> Result<(), String> {
    match PendingSignIns::load(dir) {
        Ok(mut signins) => {
            signins.cleanup_expired();
            signins.remove(provider_key);
            signins.save(dir)
        }
        Err(_) => crate::secrets::clear(dir, PENDING_SIGNINS_SECRET_KEY),
    }
}

/// Return every non-expired pending sign-in for UI resume. Expired records are
/// silently dropped (and the encrypted store updated) as a side effect.
pub fn list(dir: &Path) -> Result<Vec<PendingSignInView>, String> {
    let mut signins = PendingSignIns::load(dir)?;
    let removed = signins.cleanup_expired();
    if removed > 0 {
        signins.save(dir)?;
    }
    Ok(signins
        .pending
        .iter()
        .map(PendingSignInView::from)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_cleanup_drops_stale_records_and_keeps_fresh() {
        let mut signins = PendingSignIns::default();
        signins.insert(PendingSignIn {
            provider_key: "claude".into(),
            kind: PendingKind::ClaudePkce {
                verifier: "v".into(),
                state: "s".into(),
            },
            expires_at: Utc::now() - Duration::minutes(1),
        });
        signins.insert(PendingSignIn {
            provider_key: "codex".into(),
            kind: PendingKind::CodexDevice {
                device_auth_id: "d".into(),
                user_code: "u".into(),
                verification_url: "https://auth.openai.com/codex/device".into(),
                interval_secs: 2,
            },
            expires_at: Utc::now() + Duration::minutes(10),
        });
        assert_eq!(signins.cleanup_expired(), 1);
        assert_eq!(signins.pending.len(), 1);
        assert_eq!(signins.pending[0].provider_key, "codex");
    }

    #[test]
    fn parse_surfaces_a_storage_failure_instead_of_reading_as_empty() {
        // The criterion #133 guards: a Keystore read error must NOT look like
        // "no pending sign-ins".
        let err = PendingSignIns::parse(Err("Keystore decrypt failed".into())).unwrap_err();
        assert!(err.contains("unavailable"), "{err}");
        assert!(err.contains("Keystore decrypt failed"), "{err}");
    }

    #[test]
    fn parse_treats_absent_and_stored_blobs_normally() {
        assert_eq!(
            PendingSignIns::parse(Ok(None)).unwrap(),
            PendingSignIns::default()
        );

        let mut stored = PendingSignIns::default();
        stored.insert(PendingSignIn {
            provider_key: "claude".into(),
            kind: PendingKind::ClaudePkce {
                verifier: "v".into(),
                state: "s".into(),
            },
            expires_at: Utc::now() + Duration::minutes(10),
        });
        let json = serde_json::to_string(&stored).unwrap();
        assert_eq!(PendingSignIns::parse(Ok(Some(json))).unwrap(), stored);
    }
}
