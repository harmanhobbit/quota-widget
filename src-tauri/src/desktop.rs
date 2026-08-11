//! IPC around the per-user desktop integration.
//!
//! All of the thinking — where files go, what counts as ours, what a moved
//! AppImage means — lives in `quota_core::desktop`, which is the crate with
//! tests. This file is the shell: find the running AppImage, hand over the
//! icons the binary carries, and let the frontend ask.
//!
//! The commands exist on every platform so the invoke handler and the frontend
//! need no conditionals. Off Linux, and in any build that is not a running
//! AppImage, `desktop_integration_status` answers `None` and the UI shows
//! nothing: a Windows installer and a Nix package each already own their menu
//! entry, so there is nothing here for either of them to manage.

use crate::AppState;
use quota_core::desktop::{InstallReport, IntegrationStatus, RemovalReport};
use std::sync::Arc;
use tauri::Emitter;

#[cfg(target_os = "linux")]
mod platform {
    use quota_core::desktop::{
        DesktopIntegration, IconSource, InstallReport, IntegrationStatus, RemovalReport,
    };

    /// The icons the launcher points at, from the same files `nix/package.nix`
    /// installs into hicolor. Embedded rather than read from disk at runtime:
    /// an AppImage's mount point vanishes when the process exits, so anything
    /// left behind on the user's system has to be copied out of the binary.
    const ICONS: &[IconSource<'static>] = &[
        IconSource {
            size: "128x128",
            bytes: include_bytes!("../icons/128x128.png"),
        },
        IconSource {
            size: "32x32",
            bytes: include_bytes!("../icons/32x32.png"),
        },
    ];

    const NOT_AN_APPIMAGE: &str =
        "this build is not a running AppImage, so there is no launcher to manage";

    /// `$APPIMAGE` is set by the AppImage runtime to the image's own path, so
    /// reading the environment is both how we know we are one and how we learn
    /// the target the launcher must keep.
    fn service() -> Option<DesktopIntegration> {
        DesktopIntegration::discover(|key| std::env::var(key).ok())
    }

    pub(super) fn status() -> Option<IntegrationStatus> {
        service().map(|service| service.status())
    }

    pub(super) fn add() -> Result<InstallReport, String> {
        service()
            .ok_or_else(|| NOT_AN_APPIMAGE.to_string())?
            .install(ICONS)
            .map_err(|e| e.to_string())
    }

    pub(super) fn remove() -> Result<RemovalReport, String> {
        service()
            .ok_or_else(|| NOT_AN_APPIMAGE.to_string())?
            .remove(ICONS)
            .map_err(|e| e.to_string())
    }
}

/// Windows and macOS have no per-user launcher of this kind to manage, so the
/// commands stay present but inert rather than being conditionally registered.
#[cfg(not(target_os = "linux"))]
mod platform {
    use quota_core::desktop::{InstallReport, IntegrationStatus, RemovalReport};

    const NOT_SUPPORTED: &str = "desktop integration is a Linux AppImage feature";

    pub(super) fn status() -> Option<IntegrationStatus> {
        None
    }

    pub(super) fn add() -> Result<InstallReport, String> {
        Err(NOT_SUPPORTED.to_string())
    }

    pub(super) fn remove() -> Result<RemovalReport, String> {
        Err(NOT_SUPPORTED.to_string())
    }
}

/// What the integration currently looks like, or `None` when this build has
/// nothing to integrate — which is every non-AppImage build.
#[tauri::command]
pub fn desktop_integration_status() -> Option<IntegrationStatus> {
    platform::status()
}

/// Add the launcher and icons, or repair one whose AppImage has moved. Both are
/// the same explicit act by the user; the report says which it turned out to be.
#[tauri::command]
pub fn desktop_integration_add() -> Result<InstallReport, String> {
    platform::add()
}

/// Remove the app-owned launcher and icons. Files the user has modified are
/// left in place and reported back, so the UI can name them rather than
/// pretending the removal was complete.
#[tauri::command]
pub fn desktop_integration_remove() -> Result<RemovalReport, String> {
    platform::remove()
}

/// Record that the first-run question has been put to the user.
///
/// A dedicated command rather than a `set_config` round-trip: the popup holds
/// no editable draft of the config, and answering "Not now" must not write back
/// whatever stale copy it happens to be holding over the user's real settings.
/// It records only that they were *asked* — declining is remembered exactly as
/// firmly as accepting, and Settings keeps both explicit controls either way.
#[tauri::command]
pub async fn mark_desktop_integration_prompted(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    if config.desktop_integration_prompted {
        return Ok(());
    }
    config.desktop_integration_prompted = true;
    config.save(&state.config_dir).map_err(|e| e.to_string())?;
    // Settings holds its own draft taken at mount; the broadcast is what stops
    // it saving the pre-answer flag back over this.
    let _ = app.emit("config", &*config);
    Ok(())
}
