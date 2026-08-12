//! Per-user desktop integration for the Linux AppImage.
//!
//! An AppImage is a single file the user downloaded; nothing in the system
//! knows it exists. This module writes the one launcher and the icons that make
//! it appear in the application menu, under the user's own XDG data directory —
//! no system-wide state, no `appimaged`, no background daemon.
//!
//! Three rules shape everything here:
//!
//! 1. **The launcher targets the original AppImage.** We never copy the
//!    executable somewhere we manage, because an in-place update (the updater
//!    replaces the file at its existing path) must leave the launcher valid.
//!    A *moved* AppImage is the case that breaks, and it is offered as a repair
//!    rather than applied silently — retargeting a launcher the user may have
//!    pointed somewhere deliberately is not ours to decide.
//! 2. **We only delete what we wrote and nobody touched.** Ownership is proven,
//!    not assumed: the launcher carries a marker key *and* must byte-match what
//!    we would write for the path recorded in its own `Exec` line, and an icon
//!    must byte-match the icon we ship. Anything else is the user's file and is
//!    preserved, with the path reported so they can clean it up by hand.
//! 3. **The launcher runs under XWayland.** Native Wayland cannot position or
//!    raise its own windows (xdg-shell has no always-on-top; tao#1134), so the
//!    widget misbehaves without it. `nix/package.nix` emits the same
//!    `Exec=env GDK_BACKEND=x11 …` for the Nix build; this is its AppImage
//!    equivalent and must stay in step with it.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Basename shared by the launcher and the icons. The icon name in the
/// `.desktop` file is this stem, which is how the icon theme resolves it.
const APP_ID: &str = "quota-widget";

/// The key that claims a launcher as ours. Its presence alone proves nothing —
/// a user could copy the file — which is why intactness is a byte comparison
/// against regenerated content; the marker is what makes a *foreign* launcher
/// at our path obvious rather than merely different.
const MARKER: &str = "X-QuotaWidget-Managed=true";

/// The icon sizes we install, and the bytes for each. The caller supplies the
/// bytes (`include_bytes!` in the Tauri shell) so this crate stays free of the
/// app's assets and the tests can use tiny stand-ins.
#[derive(Debug, Clone)]
pub struct IconSource<'a> {
    /// hicolor theme directory, e.g. `128x128`.
    pub size: &'a str,
    pub bytes: &'a [u8],
}

/// What a launcher at our path currently is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum LauncherState {
    /// Nothing is installed.
    Absent,
    /// Ours, intact, and pointing at the running AppImage.
    Current,
    /// Ours and intact, but its `Exec` names a different file — the AppImage
    /// was moved or renamed since it was written. Repairable.
    Stale { target: PathBuf },
    /// A launcher we did not write, or one of ours the user has edited. Never
    /// modified and never deleted.
    UserOwned,
}

/// The whole observable state of the integration, as the UI needs it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationStatus {
    #[serde(flatten)]
    pub launcher: LauncherState,
    /// The AppImage this build is running from.
    pub appimage: PathBuf,
    /// Where the launcher lives, so the UI can name it in guidance text.
    pub desktop_file: PathBuf,
}

impl IntegrationStatus {
    /// Whether adding (or repairing) the launcher is a useful thing to offer.
    pub fn can_add(&self) -> bool {
        matches!(
            self.launcher,
            LauncherState::Absent | LauncherState::Stale { .. }
        )
    }

    pub fn can_remove(&self) -> bool {
        matches!(
            self.launcher,
            LauncherState::Current | LauncherState::Stale { .. }
        )
    }
}

/// What `install` did. `Refused` is not an error: a foreign launcher at our
/// path is a legitimate state, and the honest response is to leave it alone and
/// say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallAction {
    Created,
    Repaired,
    AlreadyCurrent,
    Refused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallReport {
    pub action: InstallAction,
    /// Files created or rewritten.
    pub written: Vec<PathBuf>,
    /// Files that already existed and were not ours to change.
    pub preserved: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemovalReport {
    pub removed: Vec<PathBuf>,
    /// Files left in place because the user had modified them (or never ours).
    pub preserved: Vec<PathBuf>,
}

impl RemovalReport {
    /// Guidance for the files removal deliberately would not touch. `None` when
    /// everything we wrote was removed, so the UI says nothing in the ordinary
    /// case rather than reassuring the user about a problem they do not have.
    pub fn manual_cleanup(&self) -> Option<String> {
        if self.preserved.is_empty() {
            return None;
        }
        let list = self
            .preserved
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!(
            "Left in place because it was edited after being installed: {list}. Delete it by hand if you no longer want it."
        ))
    }
}

/// The desktop-integration service: everything that plans and applies launcher
/// and icon changes for one running AppImage under one user data directory.
#[derive(Debug, Clone)]
pub struct DesktopIntegration {
    data_dir: PathBuf,
    appimage: PathBuf,
}

impl DesktopIntegration {
    pub fn new(data_dir: impl Into<PathBuf>, appimage: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            appimage: appimage.into(),
        }
    }

    /// The service for the process we are in, or `None` when this build is not
    /// a running AppImage and so has nothing to integrate.
    ///
    /// Takes an environment reader rather than calling `std::env` so the
    /// AppImage-detection rule is testable without mutating process state.
    /// `$APPIMAGE` is set by the AppImage runtime to the image's own path,
    /// which is exactly the target the launcher must keep.
    pub fn discover(env: impl Fn(&str) -> Option<String>) -> Option<Self> {
        let appimage = env("APPIMAGE").filter(|v| !v.is_empty())?;
        let data_dir = match env("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            // The XDG basedir default, not a guess: an unset XDG_DATA_HOME
            // means ~/.local/share by specification.
            None => PathBuf::from(env("HOME").filter(|v| !v.is_empty())?)
                .join(".local")
                .join("share"),
        };
        Some(Self::new(data_dir, appimage))
    }

    pub fn appimage(&self) -> &Path {
        &self.appimage
    }

    pub fn desktop_file(&self) -> PathBuf {
        self.data_dir
            .join("applications")
            .join(format!("{APP_ID}.desktop"))
    }

    fn icon_file(&self, size: &str) -> PathBuf {
        self.data_dir
            .join("icons")
            .join("hicolor")
            .join(size)
            .join("apps")
            .join(format!("{APP_ID}.png"))
    }

    pub fn status(&self) -> IntegrationStatus {
        IntegrationStatus {
            launcher: self.launcher_state(),
            appimage: self.appimage.clone(),
            desktop_file: self.desktop_file(),
        }
    }

    fn launcher_state(&self) -> LauncherState {
        let Ok(text) = std::fs::read_to_string(self.desktop_file()) else {
            return LauncherState::Absent;
        };
        match owned_target(&text) {
            Some(target) if target == self.appimage => LauncherState::Current,
            Some(target) => LauncherState::Stale { target },
            None => LauncherState::UserOwned,
        }
    }

    /// Add the launcher and icons, or retarget an app-owned launcher whose
    /// AppImage has moved. Both are the same write; which one it was is
    /// reported so the UI can describe what happened.
    pub fn install(&self, icons: &[IconSource<'_>]) -> std::io::Result<InstallReport> {
        let state = self.launcher_state();
        let mut report = InstallReport {
            action: match state {
                LauncherState::Absent => InstallAction::Created,
                LauncherState::Current => InstallAction::AlreadyCurrent,
                LauncherState::Stale { .. } => InstallAction::Repaired,
                LauncherState::UserOwned => InstallAction::Refused,
            },
            written: Vec::new(),
            preserved: Vec::new(),
        };
        if report.action == InstallAction::Refused {
            report.preserved.push(self.desktop_file());
            return Ok(report);
        }

        let path = self.desktop_file();
        write_atomically(&path, launcher_contents(&self.appimage).as_bytes())?;
        report.written.push(path);

        for icon in icons {
            let path = self.icon_file(icon.size);
            match std::fs::read(&path) {
                // An icon the user replaced is theirs; the launcher works with
                // whatever is under the name, so there is nothing to fix.
                Ok(existing) if existing != icon.bytes => report.preserved.push(path),
                Ok(_) => {}
                Err(_) => {
                    write_atomically(&path, icon.bytes)?;
                    report.written.push(path);
                }
            }
        }
        Ok(report)
    }

    /// Remove the launcher and icons we installed, and only those. A file that
    /// has been edited since we wrote it is left alone and reported.
    pub fn remove(&self, icons: &[IconSource<'_>]) -> std::io::Result<RemovalReport> {
        let mut report = RemovalReport {
            removed: Vec::new(),
            preserved: Vec::new(),
        };
        let path = self.desktop_file();
        match self.launcher_state() {
            LauncherState::Absent => {}
            // Both intact states are ours to delete: a stale launcher is one we
            // wrote, for an AppImage that has since moved.
            LauncherState::Current | LauncherState::Stale { .. } => {
                std::fs::remove_file(&path)?;
                report.removed.push(path);
            }
            LauncherState::UserOwned => report.preserved.push(path),
        }

        for icon in icons {
            let path = self.icon_file(icon.size);
            match std::fs::read(&path) {
                Ok(existing) if existing == icon.bytes => {
                    std::fs::remove_file(&path)?;
                    report.removed.push(path);
                }
                Ok(_) => report.preserved.push(path),
                Err(_) => {}
            }
        }
        Ok(report)
    }
}

/// The full contents of an app-owned launcher for a given AppImage. This is the
/// single definition of what "ours" means: ownership is decided by regenerating
/// this from the path a file records and comparing byte for byte, so any edit —
/// a changed category, an added key, a different `Exec` — reads as the user's.
fn launcher_contents(appimage: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Quota Widget\n\
         Comment=AI provider usage and credits in the tray\n\
         Exec={exec}\n\
         Icon={APP_ID}\n\
         Categories=Utility;\n\
         Terminal=false\n\
         {MARKER}\n",
        exec = exec_line(appimage),
    )
}

/// `env GDK_BACKEND=x11 <appimage>` — the XWayland workaround, matching the
/// `Exec` `nix/package.nix` writes for the Nix build. Without it the popup
/// cannot be positioned or raised on a Wayland session.
fn exec_line(appimage: &Path) -> String {
    format!("env GDK_BACKEND=x11 {}", quote_exec(appimage))
}

/// Desktop-entry quoting: the spec reserves a set of characters inside `Exec`,
/// and a path with a space in it is the everyday case (`~/My Apps/…`). Quote
/// unconditionally so the value round-trips through `unquote_exec` by one rule.
fn quote_exec(path: &Path) -> String {
    let escaped = path
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn unquote_exec(value: &str) -> Option<PathBuf> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            out.push(chars.next()?);
        } else {
            out.push(c);
        }
    }
    Some(PathBuf::from(out))
}

/// The AppImage an app-owned, unmodified launcher targets, or `None` when the
/// file is not ours to touch.
///
/// Deliberately strict: the marker says a file claims to be ours, and the
/// byte-comparison against regenerated content says nobody has changed it
/// since. Only both together license a delete.
///
/// The cost of that strictness is that changing `launcher_contents` orphans
/// every launcher an older build wrote — they read as `UserOwned`, so the new
/// build will neither rewrite nor delete them, and the user is told to clean up
/// a file they never touched. Treat the format as settled; if it genuinely must
/// change, this function has to accept the previous rendering too rather than
/// the new one alone.
fn owned_target(text: &str) -> Option<PathBuf> {
    if !text.lines().any(|line| line.trim() == MARKER) {
        return None;
    }
    let exec = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("Exec="))?;
    let target = unquote_exec(exec.trim().strip_prefix("env GDK_BACKEND=x11 ")?.trim())?;
    (launcher_contents(&target) == text).then_some(target)
}

/// Write through a temporary file in the same directory, as `Config::save`
/// does: a torn write must not leave a half-written launcher that the desktop
/// would then show as a broken entry.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ICONS: &[IconSource<'static>] = &[
        IconSource {
            size: "128x128",
            bytes: b"128-icon-bytes",
        },
        IconSource {
            size: "32x32",
            bytes: b"32-icon-bytes",
        },
    ];

    /// A service over a throwaway XDG data directory and a real AppImage file.
    fn fixture() -> (tempfile::TempDir, DesktopIntegration) {
        let dir = tempfile::tempdir().unwrap();
        let appimage = dir
            .path()
            .join("Downloads/QuotaWidget_0.20.3_amd64.AppImage");
        std::fs::create_dir_all(appimage.parent().unwrap()).unwrap();
        std::fs::write(&appimage, b"appimage").unwrap();
        let service = DesktopIntegration::new(dir.path().join("data"), appimage);
        (dir, service)
    }

    #[test]
    fn nothing_is_installed_before_the_user_opts_in() {
        let (_dir, service) = fixture();
        let status = service.status();
        assert_eq!(status.launcher, LauncherState::Absent);
        assert!(status.can_add());
        assert!(!status.can_remove());
        assert!(!status.desktop_file.exists());
    }

    /// Opting in writes exactly one launcher plus the icons, under the user's
    /// own data directory and nowhere else.
    #[test]
    fn opting_in_writes_the_launcher_and_icons() {
        let (_dir, service) = fixture();
        let report = service.install(ICONS).unwrap();
        assert_eq!(report.action, InstallAction::Created);
        assert!(report.preserved.is_empty());
        assert_eq!(report.written.len(), 3);

        let text = std::fs::read_to_string(service.desktop_file()).unwrap();
        // The XWayland workaround is not optional: without it the widget cannot
        // raise or position itself on a Wayland session.
        assert!(text.contains("Exec=env GDK_BACKEND=x11 "), "{text}");
        // The launcher targets the original AppImage, never a copy of it.
        assert!(
            text.contains(&service.appimage().display().to_string()),
            "{text}"
        );
        assert!(text.contains(MARKER), "{text}");
        for icon in ICONS {
            let path = service
                .data_dir
                .join(format!("icons/hicolor/{}/apps/quota-widget.png", icon.size));
            assert_eq!(std::fs::read(&path).unwrap(), icon.bytes);
        }
        assert_eq!(service.status().launcher, LauncherState::Current);
    }

    #[test]
    fn installing_twice_changes_nothing_and_says_so() {
        let (_dir, service) = fixture();
        service.install(ICONS).unwrap();
        let again = service.install(ICONS).unwrap();
        assert_eq!(again.action, InstallAction::AlreadyCurrent);
        assert!(again.preserved.is_empty());
    }

    #[test]
    fn removal_deletes_the_files_it_installed() {
        let (_dir, service) = fixture();
        service.install(ICONS).unwrap();
        let report = service.remove(ICONS).unwrap();
        assert_eq!(report.removed.len(), 3);
        assert!(report.preserved.is_empty());
        assert_eq!(report.manual_cleanup(), None);
        assert!(!service.desktop_file().exists());
        assert_eq!(service.status().launcher, LauncherState::Absent);
    }

    #[test]
    fn removing_what_was_never_installed_is_not_an_error() {
        let (_dir, service) = fixture();
        let report = service.remove(ICONS).unwrap();
        assert!(report.removed.is_empty());
        assert!(report.preserved.is_empty());
    }

    /// The rule that matters most: a launcher the user edited is theirs. It is
    /// never rewritten, never deleted, and the user is told where it is.
    #[test]
    fn a_user_edited_launcher_is_preserved_by_both_add_and_remove() {
        let (_dir, service) = fixture();
        service.install(ICONS).unwrap();
        let path = service.desktop_file();
        let edited = std::fs::read_to_string(&path)
            .unwrap()
            .replace("Name=Quota Widget", "Name=My Quota Widget");
        std::fs::write(&path, &edited).unwrap();

        assert_eq!(service.status().launcher, LauncherState::UserOwned);
        assert!(!service.status().can_add());
        assert!(!service.status().can_remove());

        let added = service.install(ICONS).unwrap();
        assert_eq!(added.action, InstallAction::Refused);
        assert_eq!(added.preserved, vec![path.clone()]);
        assert!(added.written.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);

        let removed = service.remove(ICONS).unwrap();
        assert_eq!(removed.preserved[0], path);
        assert!(!removed.removed.contains(&path));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), edited);
        let cleanup = removed.manual_cleanup().expect("guidance for a kept file");
        assert!(cleanup.contains(&path.display().to_string()), "{cleanup}");
    }

    /// A file at our path that never carried the marker is somebody else's
    /// launcher, not a modified one of ours — same protection.
    #[test]
    fn a_foreign_launcher_at_our_path_is_never_touched() {
        let (_dir, service) = fixture();
        let path = service.desktop_file();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[Desktop Entry]\nExec=something-else\n").unwrap();
        assert_eq!(service.status().launcher, LauncherState::UserOwned);
        assert_eq!(
            service.install(ICONS).unwrap().action,
            InstallAction::Refused
        );
        assert!(service.remove(ICONS).unwrap().removed.is_empty());
        assert!(path.exists());
    }

    /// An icon the user swapped for their own is theirs too: the launcher works
    /// with whatever is under that name, so there is nothing to repair.
    #[test]
    fn a_user_replaced_icon_is_preserved() {
        let (_dir, service) = fixture();
        service.install(ICONS).unwrap();
        let icon = service.icon_file("32x32");
        std::fs::write(&icon, b"my own icon").unwrap();

        let added = service.install(ICONS).unwrap();
        assert_eq!(added.preserved, vec![icon.clone()]);
        assert_eq!(std::fs::read(&icon).unwrap(), b"my own icon");

        let removed = service.remove(ICONS).unwrap();
        assert!(removed.preserved.contains(&icon));
        assert!(icon.exists());
    }

    /// A moved AppImage leaves a launcher pointing at nothing. It stays ours,
    /// it reports the target it still names, and nothing changes until repair
    /// is asked for explicitly.
    #[test]
    fn a_moved_appimage_offers_repair_rather_than_retargeting_silently() {
        let (dir, service) = fixture();
        service.install(ICONS).unwrap();
        let original = service.appimage().to_path_buf();

        let moved = dir.path().join("Apps/QuotaWidget.AppImage");
        let relocated = DesktopIntegration::new(service.data_dir.clone(), &moved);

        // Merely observing the move must not rewrite anything.
        let before = std::fs::read_to_string(relocated.desktop_file()).unwrap();
        let status = relocated.status();
        assert_eq!(
            status.launcher,
            LauncherState::Stale {
                target: original.clone()
            }
        );
        assert!(status.can_add() && status.can_remove());
        assert_eq!(
            std::fs::read_to_string(relocated.desktop_file()).unwrap(),
            before
        );

        // Repair is the explicit act, and it retargets in place.
        let report = relocated.install(ICONS).unwrap();
        assert_eq!(report.action, InstallAction::Repaired);
        let text = std::fs::read_to_string(relocated.desktop_file()).unwrap();
        assert!(text.contains(&moved.display().to_string()), "{text}");
        assert!(!text.contains(&original.display().to_string()), "{text}");
        assert_eq!(relocated.status().launcher, LauncherState::Current);
    }

    /// An in-place update (the updater path) replaces the file at the same
    /// path, so the launcher must still read as current afterwards.
    #[test]
    fn replacing_the_appimage_in_place_keeps_the_launcher_current() {
        let (_dir, service) = fixture();
        service.install(ICONS).unwrap();
        std::fs::write(service.appimage(), b"a newer build").unwrap();
        assert_eq!(service.status().launcher, LauncherState::Current);
    }

    /// A stale launcher is still ours, so removing instead of repairing is
    /// allowed — and must not leave the dead entry behind.
    #[test]
    fn a_stale_launcher_can_be_removed_as_well_as_repaired() {
        let (dir, service) = fixture();
        service.install(ICONS).unwrap();
        let relocated = DesktopIntegration::new(
            service.data_dir.clone(),
            dir.path().join("elsewhere.AppImage"),
        );
        let report = relocated.remove(ICONS).unwrap();
        assert!(report.removed.contains(&relocated.desktop_file()));
        assert!(!relocated.desktop_file().exists());
    }

    /// Paths with spaces are the everyday case for a downloaded AppImage, and
    /// an unquoted `Exec` would split into arguments.
    #[test]
    fn a_path_with_spaces_survives_the_exec_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let appimage = dir.path().join("My Apps/Quota \"Widget\".AppImage");
        let service = DesktopIntegration::new(dir.path().join("data"), &appimage);
        service.install(ICONS).unwrap();
        let text = std::fs::read_to_string(service.desktop_file()).unwrap();
        assert!(text.contains(r#"Exec=env GDK_BACKEND=x11 "#), "{text}");
        assert_eq!(service.status().launcher, LauncherState::Current);
        assert_eq!(owned_target(&text), Some(appimage));
    }

    /// The frontend branches on these strings, so the wire shape is part of the
    /// contract rather than an implementation detail of the enum.
    #[test]
    fn the_status_crosses_ipc_as_a_flat_tagged_object() {
        let (_dir, service) = fixture();
        let absent = serde_json::to_value(service.status()).unwrap();
        assert_eq!(absent["state"], "absent");
        assert_eq!(absent["appimage"], service.appimage().display().to_string());

        service.install(ICONS).unwrap();
        assert_eq!(
            serde_json::to_value(service.status()).unwrap()["state"],
            "current"
        );

        let moved = DesktopIntegration::new(service.data_dir.clone(), "/elsewhere/q.AppImage");
        let stale = serde_json::to_value(moved.status()).unwrap();
        assert_eq!(stale["state"], "stale");
        // The old target is named so the UI can say what repair would change.
        assert_eq!(stale["target"], service.appimage().display().to_string());
    }

    #[test]
    fn only_a_running_appimage_has_anything_to_integrate() {
        let env = |vars: &[(&str, &str)]| {
            let vars: Vec<(String, String)> = vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            move |key: &str| {
                vars.iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.to_string())
            }
        };
        // Not an AppImage: no launcher to manage, whatever else is set.
        assert!(DesktopIntegration::discover(env(&[("HOME", "/home/u")])).is_none());
        // An empty APPIMAGE is as absent as a missing one.
        assert!(
            DesktopIntegration::discover(env(&[("APPIMAGE", ""), ("HOME", "/home/u")])).is_none()
        );

        let explicit = DesktopIntegration::discover(env(&[
            ("APPIMAGE", "/opt/q.AppImage"),
            ("XDG_DATA_HOME", "/home/u/.share"),
            ("HOME", "/home/u"),
        ]))
        .expect("running AppImage");
        assert_eq!(explicit.appimage(), Path::new("/opt/q.AppImage"));
        assert_eq!(
            explicit.desktop_file(),
            Path::new("/home/u/.share/applications/quota-widget.desktop")
        );

        // Unset XDG_DATA_HOME means ~/.local/share by the basedir spec.
        let default = DesktopIntegration::discover(env(&[
            ("APPIMAGE", "/opt/q.AppImage"),
            ("HOME", "/home/u"),
        ]))
        .expect("running AppImage");
        assert_eq!(
            default.desktop_file(),
            Path::new("/home/u/.local/share/applications/quota-widget.desktop")
        );
    }
}
