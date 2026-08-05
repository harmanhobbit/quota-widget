//! Upstream update detection.
//!
//! The source repo is private, so the manifest and binaries live in the public
//! `quota-widget-dist` repo. This module only *detects* a newer release and
//! reports it; installing is the updater plugin's job in a later revision.
//!
//! Version comparison and manifest parsing deliberately live in `quota-core`
//! (`quota_core::update`) because that is the crate with tests. This file is
//! the shell around them: when to check, and how the result reaches the UI.

use crate::AppState;
use quota_core::update::UpdateInfo;
use std::sync::Arc;
use std::time::Duration;
use tauri::Emitter;

/// The stable URL of the public manifest. `latest/download` always resolves to
/// the newest release, so the app never needs to know a version to find one.
const MANIFEST_URL: &str = "https://github.com/harmanhobbit/quota-widget-dist/releases/latest/download/latest.json";

/// GitHub is polled rarely on purpose: an update is not time-critical, and the
/// poller's own quota cycle runs far more often than this.
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Target triple key in the manifest's `platforms` map. Only Windows is
/// published today. A build whose key is absent still learns that a newer
/// version exists — it just gets no download URL, which is the honest result
/// on *nix, where upgrading goes through whatever package manager installed
/// the app.
#[cfg(windows)]
const TARGET: &str = "windows-x86_64";
#[cfg(not(windows))]
const TARGET: &str = "linux-x86_64";

/// A dev build must never be told to "update" to a main release — its version
/// is whatever `main` last declared, so every branch build would nag forever.
/// `QUOTA_WIDGET_BRANCH` is set by CI only for non-main refs.
fn is_branch_build() -> bool {
    option_env!("QUOTA_WIDGET_BRANCH").is_some_and(|b| !b.is_empty())
}

/// Fetch and parse the manifest. Network and parse failures are indistinguishable
/// to the caller on purpose: either way there is simply no update to report.
async fn fetch() -> Option<UpdateInfo> {
    // A fresh client per check, matching how oauth/codex_oauth do it here: this
    // runs at most every six hours, so pooling would buy nothing.
    let body = reqwest::Client::new()
        .get(MANIFEST_URL)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()?;

    match UpdateInfo::from_latest_json(env!("CARGO_PKG_VERSION"), &body, TARGET) {
        Ok(info) => Some(info),
        Err(e) => {
            eprintln!("update: ignoring unusable manifest: {e}");
            None
        }
    }
}

/// Check once, store the result, and tell the frontend. Returns what it stored
/// so the manual command can answer directly instead of racing its own event.
async fn check_once(app: &tauri::AppHandle, state: &Arc<AppState>) -> Option<UpdateInfo> {
    if is_branch_build() || !state.config.read().await.check_updates {
        return None;
    }

    let info = fetch().await;
    *state.update.write().await = info.clone();
    // Emitted even when nothing is available: a check that finds no update must
    // still clear a previously shown banner.
    let _ = app.emit("update", info.clone());
    info
}

/// Check at startup and every `CHECK_INTERVAL` thereafter.
///
/// The opt-out is read inside `check_once` rather than here, so toggling
/// `check_updates` off takes effect at the next tick without restarting the
/// task, and toggling it back on resumes without one either.
pub fn spawn(app: tauri::AppHandle, state: Arc<AppState>) {
    if is_branch_build() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        loop {
            check_once(&app, &state).await;
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

/// The last known status, without hitting the network. The frontend calls this
/// on mount, when a fetch would make opening Settings feel slow.
#[tauri::command]
pub async fn update_status(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Option<UpdateInfo>, String> {
    Ok(state.update.read().await.clone())
}

/// Force a check now, for the Settings "Check now" button.
#[tauri::command]
pub async fn check_update_now(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Option<UpdateInfo>, String> {
    if is_branch_build() {
        return Ok(None);
    }
    // Deliberately bypasses the config opt-out: pressing the button *is* the
    // consent, and the button stays useful for someone who keeps checks off.
    let info = fetch().await;
    *state.update.write().await = info.clone();
    let _ = app.emit("update", info.clone());
    Ok(info)
}
