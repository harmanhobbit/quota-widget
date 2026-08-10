//! Upstream update detection.
//!
//! The source repo is private, so the manifest and binaries live in the public
//! `quota-widget-dist` repo. This module only *detects* a newer release and
//! reports it; the updater plugin installs releases only for bundle types it
//! can safely replace.
//!
//! Version comparison and manifest parsing deliberately live in `quota-core`
//! (`quota_core::update`) because that is the crate with tests. This file is
//! the shell around them: when to check, and how the result reaches the UI.

use crate::AppState;
use quota_core::update::UpdateInfo;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;
use tauri::Emitter;

/// The stable URL of the public manifest. `latest/download` always resolves to
/// the newest release, so the app never needs to know a version to find one.
const MANIFEST_URL: &str =
    "https://github.com/harmanhobbit/quota-widget-dist/releases/latest/download/latest.json";

/// GitHub is polled rarely on purpose: an update is not time-critical, and the
/// poller's own quota cycle runs far more often than this.
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// Target triple key in the manifest's `platforms` map. Windows installers and
/// Linux AppImages are published; a build whose key is absent still learns that
/// a newer version exists — it just gets no download URL, which remains the
/// honest result for package-managed installs such as Nix.
#[cfg(windows)]
const TARGET: &str = "windows-x86_64";
#[cfg(not(windows))]
const TARGET: &str = "linux-x86_64";

/// What Settings needs to know about a detected release. A platform download
/// and an installable running bundle are separate facts: a portable Windows
/// executable can download the NSIS installer but cannot replace itself.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct UpdateStatus {
    #[serde(flatten)]
    info: UpdateInfo,
    installable: bool,
}

fn status_for(info: Option<UpdateInfo>, installable: bool) -> Option<UpdateStatus> {
    info.map(|info| UpdateStatus { info, installable })
}

/// The updater uses Tauri's bundle type to select an installer. It returns
/// `None` for a portable executable, which is exactly the signal Settings
/// needs to avoid offering an action that would fail.
fn current_status(info: Option<UpdateInfo>) -> Option<UpdateStatus> {
    status_for(info, tauri::utils::platform::bundle_type().is_some())
}

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
    let _ = app.emit("update", current_status(info.clone()));
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
) -> Result<Option<UpdateStatus>, String> {
    Ok(current_status(state.update.read().await.clone()))
}

/// Force a check now, for the Settings "Check now" button.
#[tauri::command]
pub async fn check_update_now(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Option<UpdateStatus>, String> {
    if is_branch_build() {
        return Ok(None);
    }
    // Deliberately bypasses the config opt-out: pressing the button *is* the
    // consent, and the button stays useful for someone who keeps checks off.
    let info = fetch().await;
    *state.update.write().await = info.clone();
    let status = current_status(info);
    let _ = app.emit("update", status.clone());
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update() -> UpdateInfo {
        UpdateInfo {
            current: "0.20.0".into(),
            latest: "0.20.3".into(),
            url: Some("https://example.test/setup.exe".into()),
            notes: String::new(),
            pub_date: "2026-08-10T00:00:00Z".into(),
            available: true,
        }
    }

    #[test]
    fn status_keeps_download_and_installability_independent() {
        let portable = status_for(Some(update()), false).expect("status for available update");
        assert!(portable.info.url.is_some());
        assert!(!portable.installable);

        let installed = status_for(Some(update()), true).expect("status for available update");
        assert!(installed.info.url.is_some());
        assert!(installed.installable);
    }

    #[test]
    fn status_stays_absent_without_an_update() {
        assert_eq!(status_for(None, true), None);
    }
}
