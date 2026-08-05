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

/// The status crossing quota-core, AppState, IPC, and Settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    /// Download for *this* platform, when the release published one. `None`
    /// means a newer version exists but not in a form this build can install —
    /// the *nix case, where releases are Windows-only and upgrading goes
    /// through the package manager the app was installed with.
    pub url: Option<String>,
    pub notes: String,
    pub pub_date: String,
    pub available: bool,
}

impl UpdateInfo {
    /// Parse the updater-compatible manifest and select this build's target.
    pub fn from_latest_json(
        current: impl Into<String>,
        json: &str,
        target: &str,
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
        let url = manifest.platforms.get(target).map(|p| p.url.clone());

        Ok(Self {
            available: is_newer(&current, &manifest.version),
            current,
            latest: manifest.version,
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

    const MANIFEST: &str = r#"{
        "version":"0.10.0",
        "notes":"See the release page.",
        "pub_date":"2026-08-05T09:21:10Z",
        "platforms":{"windows-x86_64":{"signature":"signature","url":"https://example.test/setup.exe"}},
        "future_plugin_field":true
    }"#;

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

    #[test]
    fn parses_manifest_and_ignores_unknown_fields() {
        let info = UpdateInfo::from_latest_json("0.9.0", MANIFEST, "windows-x86_64").unwrap();
        assert_eq!(
            info,
            UpdateInfo {
                current: "0.9.0".into(),
                latest: "0.10.0".into(),
                url: Some("https://example.test/setup.exe".into()),
                notes: "See the release page.".into(),
                pub_date: "2026-08-05T09:21:10Z".into(),
                available: true,
            }
        );
    }

    /// The *nix case: releases are Windows-only, so this build finds no
    /// download for itself but must still learn that a newer version exists.
    #[test]
    fn reports_the_version_when_this_platform_has_no_download() {
        let info = UpdateInfo::from_latest_json("0.9.0", MANIFEST, "linux-x86_64").unwrap();
        assert!(info.available);
        assert_eq!(info.latest, "0.10.0");
        assert_eq!(info.url, None);
    }

    /// ...and an unavailable update stays unavailable regardless of platform,
    /// so a missing download can never be mistaken for one.
    #[test]
    fn no_download_does_not_invent_an_update() {
        let info = UpdateInfo::from_latest_json("0.10.0", MANIFEST, "linux-x86_64").unwrap();
        assert!(!info.available);
        assert_eq!(info.url, None);
    }

    #[test]
    fn rejects_bad_manifest_values() {
        assert!(matches!(
            UpdateInfo::from_latest_json("0.9.0", r#"{"version":"v0.10.0"}"#, "windows-x86_64"),
            Err(UpdateError::Json(_))
        ));
        assert!(matches!(
            UpdateInfo::from_latest_json(
                "0.9.0",
                &MANIFEST.replace("0.10.0", "v0.10.0"),
                "windows-x86_64"
            ),
            Err(UpdateError::InvalidVersion(_))
        ));
        assert!(matches!(
            UpdateInfo::from_latest_json("0.9.0", &MANIFEST.replace("09:21:10Z", "whenever"), "windows-x86_64"),
            Err(UpdateError::InvalidPubDate(_))
        ));
    }
}
