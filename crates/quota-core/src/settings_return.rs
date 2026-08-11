//! Where Settings goes when it exits.
//!
//! The Settings return state is captured once per Settings visit and lives only
//! for that visit — it is never written to the config file, because "which
//! window was on screen a moment ago" is not a preference the user set.
//!
//! It lives here rather than in the Tauri shell so the decision itself is
//! testable: the shell supplies two booleans about what is currently visible,
//! and everything downstream (the `navigate` payload, the frontend's exit
//! action) is a consequence of the value chosen here.

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsReturn {
    /// Back to the usage popup — the full window stays on screen.
    #[default]
    Popup,
    /// Back to the mini summary, pinned or transient as it was.
    Mini,
    /// Back to no visible window at all.
    Hidden,
}

impl SettingsReturn {
    /// Decide the return state for a tray-menu Settings entry.
    ///
    /// It must be asked *before* the full window is shown: showing it hides the
    /// mini summary, after which nothing on screen distinguishes "the tray
    /// summary was open" from "nothing was open". The popup wins a tie because
    /// Settings is about to occupy that same window, so leaving it up is the
    /// outcome that changes least.
    pub fn for_tray_entry(mini_visible: bool, popup_visible: bool) -> Self {
        if popup_visible {
            SettingsReturn::Popup
        } else if mini_visible {
            SettingsReturn::Mini
        } else {
            SettingsReturn::Hidden
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_settings_over_a_visible_mini_summary_returns_to_it() {
        assert_eq!(
            SettingsReturn::for_tray_entry(true, false),
            SettingsReturn::Mini
        );
    }

    #[test]
    fn tray_settings_over_the_usage_popup_returns_to_the_popup() {
        assert_eq!(
            SettingsReturn::for_tray_entry(false, true),
            SettingsReturn::Popup
        );
        // The popup hides the mini as it opens, so both-visible is not a state
        // the shell produces — but if it ever did, Settings replaces the popup
        // and returning there is the smaller move.
        assert_eq!(
            SettingsReturn::for_tray_entry(true, true),
            SettingsReturn::Popup
        );
    }

    #[test]
    fn tray_settings_with_nothing_on_screen_returns_to_nothing() {
        assert_eq!(
            SettingsReturn::for_tray_entry(false, false),
            SettingsReturn::Hidden
        );
    }

    /// The value crosses the navigation boundary as JSON, and the frontend
    /// compares it against these exact strings.
    #[test]
    fn serializes_as_the_strings_the_frontend_switches_on() {
        let json = |r: SettingsReturn| serde_json::to_string(&r).unwrap();
        assert_eq!(json(SettingsReturn::Popup), "\"popup\"");
        assert_eq!(json(SettingsReturn::Mini), "\"mini\"");
        assert_eq!(json(SettingsReturn::Hidden), "\"hidden\"");
    }
}
