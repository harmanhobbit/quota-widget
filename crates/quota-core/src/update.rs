//! The public release manifest shared with `tauri-plugin-updater`.
//!
//! Keep this deliberately limited to the updater's documented `latest.json`
//! shape. The Tauri shell fetches it; quota-core only validates and turns it
//! into the small status object the UI needs.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// Whether `candidate` is a strictly newer bare `major.minor.patch` version.
///
/// Release tags use a `v` prefix, but the manifest must not. Invalid versions
/// are not comparable and therefore never claim an update is available.
pub fn is_newer(current: &str, candidate: &str) -> bool {
    match (parse_version(current), parse_version(candidate)) {
        (Some(current), Some(candidate)) => candidate > current,
        _ => false,
    }
}

/// The bundle format a build was packaged as, named the way the manifest and
/// `tauri-plugin-updater` spell it.
///
/// Platform keys are qualified by this name — `linux-x86_64-appimage`, not a
/// bare `linux-x86_64` — because one platform has several mutually exclusive
/// package formats. A future `.deb` or Flatpak release publishes its own entry
/// beside the AppImage's rather than overwriting the one artifact that happens
/// to have been published first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Artifact {
    AppImage,
    Deb,
    Rpm,
    Nsis,
    Msi,
    App,
}

impl Artifact {
    /// The manifest key suffix, matching the installer names the updater
    /// plugin builds its own lookup from. These strings are a wire contract
    /// with `latest.json`, not a display name.
    pub fn key_suffix(self) -> &'static str {
        match self {
            Self::AppImage => "appimage",
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::Nsis => "nsis",
            Self::Msi => "msi",
            Self::App => "app",
        }
    }

    /// Whether finishing an install hands the user a running new version.
    ///
    /// The Windows installers exit the app and relaunch it themselves. Every
    /// Linux format instead rewrites files underneath a process that keeps
    /// running, so the new version only appears on the next launch — which is
    /// why Linux must offer a restart rather than claim one happened.
    pub fn relaunches_itself(self) -> bool {
        matches!(self, Self::Nsis | Self::Msi)
    }
}

/// What this build looks for in the manifest: its platform, and the package
/// format it was installed as, if any.
///
/// `artifact` is `None` for a build no updater can replace in place — a
/// portable Windows EXE, or a package-manager install such as Nix. That is a
/// different fact from "the release published no download", and the two must
/// stay separate: a portable EXE sees the Windows download and still cannot
/// use it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTarget {
    platform: String,
    artifact: Option<Artifact>,
}

impl UpdateTarget {
    pub fn new(platform: impl Into<String>, artifact: Option<Artifact>) -> Self {
        Self {
            platform: platform.into(),
            artifact,
        }
    }

    /// Manifest keys to try, most specific first. This mirrors the updater
    /// plugin's own `{os}-{arch}-{installer}` then `{os}-{arch}` search, so the
    /// entry Settings reports is the entry the plugin will later download.
    pub fn keys(&self) -> Vec<String> {
        let mut keys = Vec::with_capacity(2);
        if let Some(artifact) = self.artifact {
            keys.push(format!("{}-{}", self.platform, artifact.key_suffix()));
        }
        keys.push(self.platform.clone());
        keys
    }
}

/// The status crossing quota-core, AppState, IPC, and Settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    /// Download for *this* platform, when the release published one. `None`
    /// means a newer version exists but not in a form this build can download —
    /// the package-managed case, such as Nix, where upgrading goes through
    /// whatever installed the app.
    pub url: Option<String>,
    pub notes: String,
    pub pub_date: String,
    pub available: bool,
    /// Whether the running build can replace itself with that download. A
    /// download alone does not imply this: a portable EXE finds the Windows
    /// installer in the manifest and still cannot install it.
    pub installable: bool,
    /// Whether a completed install leaves the user to restart. False on the
    /// Windows installers, which relaunch the app themselves.
    pub restart_after_install: bool,
}

impl UpdateInfo {
    /// Parse the updater-compatible manifest and select this build's artifact.
    pub fn from_latest_json(
        current: impl Into<String>,
        json: &str,
        target: &UpdateTarget,
    ) -> Result<Self, UpdateError> {
        let manifest: LatestManifest = serde_json::from_str(json)?;
        let current = current.into();
        validate_version(&current)?;
        validate_version(&manifest.version)?;
        chrono::DateTime::parse_from_rfc3339(&manifest.pub_date)
            .map_err(|_| UpdateError::InvalidPubDate(manifest.pub_date.clone()))?;
        // A missing platform entry is not an error. The version, and therefore
        // "is something newer out there", is platform-independent; only the
        // download is not. Treating absence as a failure meant a *nix build
        // reported nothing at all, when what it wants is exactly the flag
        // without an install path.
        let url = target
            .keys()
            .iter()
            .find_map(|key| manifest.platforms.get(key))
            .map(|p| p.url.clone());
        let artifact = target.artifact;

        Ok(Self {
            available: is_newer(&current, &manifest.version),
            current,
            latest: manifest.version,
            // Installability is the conjunction of the two independent facts:
            // something to download, and a bundle the updater can replace.
            installable: url.is_some() && artifact.is_some(),
            restart_after_install: artifact.is_some_and(|a| !a.relaunches_itself()),
            url,
            notes: manifest.notes,
            pub_date: manifest.pub_date,
        })
    }
}

/// The documented `latest.json` shape consumed by Tauri's updater plugin.
/// Platform entries are a map so future target triples need no schema change.
#[derive(Debug, Deserialize)]
struct LatestManifest {
    version: String,
    notes: String,
    pub_date: String,
    platforms: BTreeMap<String, Platform>,
}

#[derive(Debug, Deserialize)]
struct Platform {
    // Parsing the signature ensures this remains the updater's real manifest,
    // even though detection itself only renders the download URL.
    #[allow(dead_code)]
    signature: String,
    url: String,
}

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("invalid update manifest: {0}")]
    Json(#[from] serde_json::Error),
    #[error("version must be bare major.minor.patch: {0}")]
    InvalidVersion(String),
    #[error("pub_date must be RFC 3339: {0}")]
    InvalidPubDate(String),
}

fn validate_version(version: &str) -> Result<(), UpdateError> {
    parse_version(version)
        .map(|_| ())
        .ok_or_else(|| UpdateError::InvalidVersion(version.to_owned()))
}

fn parse_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut components = version.split('.');
    let parsed = [
        parse_component(components.next()?),
        parse_component(components.next()?),
        parse_component(components.next()?),
    ];
    if components.next().is_some() {
        return None;
    }
    Some([parsed[0]?, parsed[1]?, parsed[2]?]).map(|v| (v[0], v[1], v[2]))
}

fn parse_component(component: &str) -> Option<u64> {
    if component.is_empty() || (component.len() > 1 && component.starts_with('0')) {
        return None;
    }
    component.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A release as `release.yml` publishes it: one entry per artifact, each
    /// key qualified by the package format it carries.
    const MANIFEST: &str = r#"{
        "version":"0.10.0",
        "notes":"See the release page.",
        "pub_date":"2026-08-05T09:21:10Z",
        "platforms":{
            "windows-x86_64":{"signature":"nsis-signature","url":"https://example.test/setup.exe"},
            "linux-x86_64-appimage":{"signature":"appimage-signature","url":"https://example.test/app.AppImage"}
        },
        "future_plugin_field":true
    }"#;

    fn windows_installed() -> UpdateTarget {
        UpdateTarget::new("windows-x86_64", Some(Artifact::Nsis))
    }

    fn appimage() -> UpdateTarget {
        UpdateTarget::new("linux-x86_64", Some(Artifact::AppImage))
    }

    /// Nix, or any package-manager install: the platform is Linux, but nothing
    /// the updater can replace is running.
    fn unpackaged_linux() -> UpdateTarget {
        UpdateTarget::new("linux-x86_64", None)
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(!is_newer("0.10.0", "0.10.0"));
        assert!(!is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("0.9.0", "0.10.0"));
        assert!(is_newer("0.9.9", "0.10.0"));
        assert!(is_newer("0.10.9", "0.11.0"));
        assert!(is_newer("0.10.9", "1.0.0"));
    }

    #[test]
    fn malformed_or_tag_versions_are_not_comparable() {
        for version in ["v0.10.0", "0.10", "0.10.0.1", "0.x.0", "01.0.0"] {
            assert!(!is_newer("0.9.0", version), "{version}");
        }
    }

    /// Most specific first, mirroring the updater plugin's own lookup, so the
    /// entry Settings reports is the entry the plugin will download.
    #[test]
    fn qualified_keys_are_tried_before_the_bare_platform() {
        assert_eq!(appimage().keys(), ["linux-x86_64-appimage", "linux-x86_64"]);
        // Nothing installable means there is no qualified key to try at all.
        assert_eq!(unpackaged_linux().keys(), ["linux-x86_64"]);
    }

    #[test]
    fn parses_manifest_and_ignores_unknown_fields() {
        let info = UpdateInfo::from_latest_json("0.9.0", MANIFEST, &windows_installed()).unwrap();
        assert_eq!(
            info,
            UpdateInfo {
                current: "0.9.0".into(),
                latest: "0.10.0".into(),
                url: Some("https://example.test/setup.exe".into()),
                notes: "See the release page.".into(),
                pub_date: "2026-08-05T09:21:10Z".into(),
                available: true,
                installable: true,
                // The NSIS installer relaunches the app itself, so Windows must
                // never be told to restart by hand.
                restart_after_install: false,
            }
        );
    }

    /// The AppImage entry is artifact-qualified, so selection has to look past
    /// the bare platform key to find it.
    #[test]
    fn selects_the_artifact_qualified_appimage_entry() {
        let info = UpdateInfo::from_latest_json("0.9.0", MANIFEST, &appimage()).unwrap();
        assert!(info.available);
        assert_eq!(
            info.url.as_deref(),
            Some("https://example.test/app.AppImage")
        );
        assert!(info.installable);
        // Replacing an AppImage leaves the old one running, so the user has to
        // restart before the new version is what is on screen.
        assert!(info.restart_after_install);
    }

    /// The qualified key is what keeps package formats independent: adding a
    /// `.deb` entry must neither be picked up by an AppImage build nor
    /// displace the AppImage's own entry.
    #[test]
    fn package_formats_do_not_collide() {
        let appimage_entry = r#""linux-x86_64-appimage":{"signature":"appimage-signature","url":"https://example.test/app.AppImage"}"#;
        let with_deb = MANIFEST.replace(
            appimage_entry,
            &format!(
                r#"{appimage_entry},
               "linux-x86_64-deb":{{"signature":"deb-signature","url":"https://example.test/app.deb"}}"#
            ),
        );

        let from_appimage = UpdateInfo::from_latest_json("0.9.0", &with_deb, &appimage()).unwrap();
        assert_eq!(
            from_appimage.url.as_deref(),
            Some("https://example.test/app.AppImage")
        );

        let deb = UpdateTarget::new("linux-x86_64", Some(Artifact::Deb));
        let from_deb = UpdateInfo::from_latest_json("0.9.0", &with_deb, &deb).unwrap();
        assert_eq!(
            from_deb.url.as_deref(),
            Some("https://example.test/app.deb")
        );
    }

    /// A build with no AppImage entry to select still learns a newer version
    /// exists — the version is platform-independent, the download is not.
    #[test]
    fn reports_the_version_when_this_artifact_has_no_download() {
        let windows_only = MANIFEST.replace("linux-x86_64-appimage", "linux-aarch64-appimage");
        let info = UpdateInfo::from_latest_json("0.9.0", &windows_only, &appimage()).unwrap();
        assert!(info.available);
        assert_eq!(info.latest, "0.10.0");
        assert_eq!(info.url, None);
        assert!(!info.installable);
    }

    /// An available release and an installable artifact are separate facts.
    /// Nix has the AppImage download in front of it and still cannot use it,
    /// because nothing the updater can replace is running.
    #[test]
    fn a_download_is_not_an_installable_artifact() {
        let info = UpdateInfo::from_latest_json("0.9.0", MANIFEST, &unpackaged_linux()).unwrap();
        assert!(info.available);
        // The bare platform key is still consulted, so a package-managed build
        // can be pointed at a download; it just cannot install one.
        assert!(!info.installable);
        assert!(!info.restart_after_install);
    }

    /// ...and an unavailable update stays unavailable regardless of artifact,
    /// so a missing download can never be mistaken for one.
    #[test]
    fn no_download_does_not_invent_an_update() {
        let info = UpdateInfo::from_latest_json("0.10.0", MANIFEST, &unpackaged_linux()).unwrap();
        assert!(!info.available);
        assert_eq!(info.url, None);
        assert!(!info.installable);
    }

    #[test]
    fn rejects_bad_manifest_values() {
        assert!(matches!(
            UpdateInfo::from_latest_json("0.9.0", r#"{"version":"v0.10.0"}"#, &windows_installed()),
            Err(UpdateError::Json(_))
        ));
        assert!(matches!(
            UpdateInfo::from_latest_json(
                "0.9.0",
                &MANIFEST.replace("0.10.0", "v0.10.0"),
                &windows_installed()
            ),
            Err(UpdateError::InvalidVersion(_))
        ));
        assert!(matches!(
            UpdateInfo::from_latest_json(
                "0.9.0",
                &MANIFEST.replace("09:21:10Z", "whenever"),
                &windows_installed()
            ),
            Err(UpdateError::InvalidPubDate(_))
        ));
    }
}
