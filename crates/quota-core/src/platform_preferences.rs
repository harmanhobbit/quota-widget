//! Platform preferences: behaviour that exists on only one host, such as
//! desktop window/tray/AppImage presentation. See CONTEXT.md "Platform
//! preferences" and docs/adr/0006-….
//!
//! Unlike [`crate::shared_config::SharedConfig`], this is *derived* data in
//! the corruption-policy sense used at the config boundary: nothing here
//! survives a fresh install (every field has a sensible default), and losing
//! it costs the user a few re-clicked toggles, not re-entered accounts or
//! secrets. A file that cannot be read or parsed is therefore replaced with
//! defaults rather than kept aside and blocking every future save — see
//! `SharedConfig::load` for the contrasting policy this deliberately does not
//! apply.

use crate::config::MiniAnchor;
use serde::{Deserialize, Serialize};
use std::path::Path;

const FILE_NAME: &str = "platform-preferences.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PlatformPreferences {
    /// How often the desktop poller schedules the shared refresh operation.
    /// Scheduling is a host responsibility (see `crate::refresh`), so this
    /// knob lives here rather than in shared configuration.
    pub poll_interval_secs: u64,
    pub autostart: bool,
    /// Hide the popup when it loses focus (click-away dismiss). Off by
    /// default: on Windows, starting a title-bar drag briefly drops focus,
    /// so this fights window dragging (tauri#10767) — the poller grants a
    /// grace period after a drag press, but Esc/✕/tray remain the reliable
    /// ways to dismiss.
    pub hide_on_blur: bool,
    /// Show usage bars in the compact tray-click summary.
    pub mini_summary_bars: bool,
    /// Let scrolling over a window fade its painted shell. The level itself is
    /// deliberately ephemeral so reopening the widget never leaves it hidden.
    pub scroll_opacity: bool,
    /// Swap which wheel direction fades. The webview reports wheel deltas with
    /// the platform's own sign convention, so the gesture that fades on one OS
    /// restores on another — this lets the user match it to their machine.
    pub scroll_opacity_invert: bool,
    /// Check the public distribution manifest for a newer released build.
    pub check_updates: bool,
    /// Whether the AppImage desktop-integration question has been put to the
    /// user. Records that they were *asked*, not what they answered — see
    /// `crate::config::Config::desktop_integration_prompted`, whose doc this
    /// mirrors.
    pub desktop_integration_prompted: bool,
    /// Whether Android has put the one POST_NOTIFICATIONS runtime-permission
    /// request to the user (issue #112). Records that the request was *issued*,
    /// not what was answered — a denial is remembered as firmly as a grant, so
    /// the user is never re-prompted; Settings is the recovery path. See
    /// `crate::config::Config::notification_permission_requested`, whose doc
    /// this mirrors.
    pub notification_permission_requested: bool,
    /// Where the mini summary is pinned.
    pub mini_anchor: MiniAnchor,
}

impl Default for PlatformPreferences {
    fn default() -> Self {
        Self {
            poll_interval_secs: 60,
            autostart: false,
            hide_on_blur: false,
            mini_summary_bars: true,
            scroll_opacity: true,
            scroll_opacity_invert: false,
            check_updates: true,
            desktop_integration_prompted: false,
            notification_permission_requested: false,
            mini_anchor: MiniAnchor::default(),
        }
    }
}

impl PlatformPreferences {
    /// The fields carried over from a pre-split `config.json`, for the
    /// one-time migration. Everything else in that file was shared
    /// configuration and belongs to `SharedConfig::from_legacy` instead.
    pub fn from_legacy(config: &crate::config::Config) -> Self {
        Self {
            poll_interval_secs: config.poll_interval_secs,
            autostart: config.autostart,
            hide_on_blur: config.hide_on_blur,
            mini_summary_bars: config.mini_summary_bars,
            scroll_opacity: config.scroll_opacity,
            scroll_opacity_invert: config.scroll_opacity_invert,
            check_updates: config.check_updates,
            desktop_integration_prompted: config.desktop_integration_prompted,
            notification_permission_requested: config.notification_permission_requested,
            mini_anchor: config.mini_anchor.clone(),
        }
    }

    /// Reads `platform-preferences.json`. Unlike `SharedConfig::load`, there
    /// is no recovery state to report: a missing, malformed or unreadable
    /// file is indistinguishable from "nothing configured yet" and simply
    /// yields defaults. The next save replaces it — derived data never blocks
    /// on a file it cannot make sense of.
    pub fn load(dir: &Path) -> Self {
        let path = dir.join(FILE_NAME);
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes the preferences, atomically. Always succeeds through to a
    /// consistent file on disk (or leaves the previous one intact on I/O
    /// failure) — there is no refusal path, because there is nothing here
    /// worth refusing to overwrite.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self).expect("preferences serialize");
        let path = dir.join(FILE_NAME);
        let tmp = dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Corner;

    #[test]
    fn defaults_match_what_every_build_before_the_split_did() {
        let prefs = PlatformPreferences::default();
        assert_eq!(prefs.poll_interval_secs, 60);
        assert!(!prefs.autostart);
        assert!(prefs.mini_summary_bars);
        assert!(prefs.check_updates);
        assert_eq!(prefs.mini_anchor.corner, Corner::BottomRight);
    }

    #[test]
    fn round_trips_through_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let prefs = PlatformPreferences {
            poll_interval_secs: 120,
            autostart: true,
            mini_anchor: MiniAnchor {
                monitor: Some("DP-1".into()),
                corner: Corner::TopLeft,
            },
            ..Default::default()
        };
        prefs.save(dir.path()).unwrap();
        assert_eq!(PlatformPreferences::load(dir.path()), prefs);
    }

    #[test]
    fn a_missing_file_loads_as_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            PlatformPreferences::load(dir.path()),
            PlatformPreferences::default()
        );
    }

    /// The whole point of "derived, not preserved": a corrupt file must not
    /// block saving, unlike `SharedConfig`'s refusal on the same condition.
    #[test]
    fn a_corrupt_file_loads_as_defaults_and_does_not_block_the_next_save() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), "{not json").unwrap();
        assert_eq!(
            PlatformPreferences::load(dir.path()),
            PlatformPreferences::default()
        );

        let prefs = PlatformPreferences {
            poll_interval_secs: 30,
            ..Default::default()
        };
        prefs.save(dir.path()).unwrap();
        assert_eq!(PlatformPreferences::load(dir.path()), prefs);
    }

    #[test]
    fn migrating_from_a_legacy_config_carries_only_the_platform_fields() {
        let mut legacy = crate::config::Config {
            poll_interval_secs: 90,
            hide_on_blur: true,
            ..Default::default()
        };
        legacy.mini_anchor = MiniAnchor {
            monitor: Some("DP-2".into()),
            corner: Corner::TopRight,
        };
        let prefs = PlatformPreferences::from_legacy(&legacy);
        assert_eq!(prefs.poll_interval_secs, 90);
        assert!(prefs.hide_on_blur);
        assert_eq!(prefs.mini_anchor, legacy.mini_anchor);
    }
}
