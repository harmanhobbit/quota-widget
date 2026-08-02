use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Thresholds {
    pub warn_pct: f64,
    pub critical_pct: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            warn_pct: 80.0,
            critical_pct: 95.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AlertToggles {
    pub toast: bool,
    pub tray_color: bool,
    pub auto_popup: bool,
}

impl Default for AlertToggles {
    fn default() -> Self {
        Self {
            toast: true,
            tray_color: true,
            auto_popup: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProviderConfig {
    /// Adapter kind. Missing means the original map key, preserving old files.
    pub kind: Option<String>,
    /// User-facing account label. Missing uses the adapter's standard name.
    pub label: Option<String>,
    pub enabled: bool,
    /// Whether this account's mini-summary headline contributes to the tray
    /// icon's status and gauge fill. On by default; turn off to keep the
    /// account visible without letting its selected headline colour the tray.
    pub in_tray: bool,
    /// Overrides the global thresholds when set.
    pub thresholds: Option<Thresholds>,
    /// Overrides the global alert toggles when set.
    pub alerts: Option<AlertToggles>,
    /// Warn when a credit balance drops to/below this value.
    pub low_balance_warn: Option<f64>,
    /// Headline shown for this account in the compact tray-click summary.
    /// `None` preserves the automatic worst-window/credits selection; the
    /// string `"none"` explicitly omits the account from that summary.
    pub mini_summary_metric: Option<String>,
    /// Provider-specific knobs (endpoint overrides, token price, …).
    pub settings: serde_json::Map<String, serde_json::Value>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            kind: None,
            label: None,
            enabled: false,
            in_tray: true,
            thresholds: None,
            alerts: None,
            low_balance_warn: None,
            mini_summary_metric: None,
            settings: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Reserved for future migrations. Old, unversioned files deserialize as 0.
    pub version: u32,
    pub poll_interval_secs: u64,
    pub thresholds: Thresholds,
    pub alerts: AlertToggles,
    pub autostart: bool,
    /// Hide the popup when it loses focus (click-away dismiss). Off by
    /// default: on Windows, starting a title-bar drag briefly drops focus,
    /// so this fights window dragging (tauri#10767) — the poller grants a
    /// grace period after a drag press, but Esc/✕/tray remain the reliable
    /// ways to dismiss.
    pub hide_on_blur: bool,
    /// Show usage bars in the compact tray-click summary.
    pub mini_summary_bars: bool,
    /// Account iteration order is the user-selected display order everywhere.
    pub providers: IndexMap<String, ProviderConfig>,
}

impl Default for Config {
    fn default() -> Self {
        let mut providers = IndexMap::new();
        // Claude and Codex work out of the box when their CLIs are logged in.
        providers.insert(
            "claude".into(),
            ProviderConfig {
                enabled: true,
                ..Default::default()
            },
        );
        providers.insert(
            "codex".into(),
            ProviderConfig {
                enabled: true,
                ..Default::default()
            },
        );
        providers.insert("openrouter".into(), ProviderConfig::default());
        providers.insert("hermes".into(), ProviderConfig::default());
        Self {
            version: 1,
            poll_interval_secs: 60,
            thresholds: Thresholds::default(),
            alerts: AlertToggles::default(),
            autostart: false,
            hide_on_blur: false,
            mini_summary_bars: true,
            providers,
        }
    }
}

impl Config {
    pub fn effective_thresholds(&self, provider_id: &str) -> Thresholds {
        self.providers
            .get(provider_id)
            .and_then(|p| p.thresholds.clone())
            .unwrap_or_else(|| self.thresholds.clone())
    }

    pub fn effective_alerts(&self, provider_id: &str) -> AlertToggles {
        self.providers
            .get(provider_id)
            .and_then(|p| p.alerts.clone())
            .unwrap_or_else(|| self.alerts.clone())
    }

    /// Whether this account's chosen mini-summary value contributes to the
    /// tray icon. Unknown providers (config written by a newer build) default
    /// to counting.
    pub fn counts_in_tray(&self, provider_id: &str) -> bool {
        self.providers
            .get(provider_id)
            .map(|p| p.in_tray)
            .unwrap_or(true)
    }

    pub fn provider_setting(&self, provider_id: &str, key: &str) -> Option<serde_json::Value> {
        self.providers.get(provider_id)?.settings.get(key).cloned()
    }

    pub fn load(dir: &Path) -> Self {
        let path = dir.join("config.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self).expect("config serializes");
        let path = dir.join("config.json");
        let tmp = dir.join("config.json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.poll_interval_secs = 120;
        cfg.providers.get_mut("openrouter").unwrap().enabled = true;
        cfg.save(dir.path()).unwrap();

        let loaded = Config::load(dir.path());
        assert_eq!(loaded, cfg);
        assert!(loaded.providers["openrouter"].enabled);
        assert_eq!(loaded.effective_thresholds("claude").warn_pct, 80.0);
    }

    #[test]
    fn missing_or_corrupt_file_yields_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Config::load(dir.path()), Config::default());
        std::fs::write(dir.path().join("config.json"), "{not json").unwrap();
        assert_eq!(Config::load(dir.path()), Config::default());
    }

    #[test]
    fn per_provider_overrides_win() {
        let mut cfg = Config::default();
        cfg.providers.get_mut("claude").unwrap().thresholds = Some(Thresholds {
            warn_pct: 50.0,
            critical_pct: 75.0,
        });
        assert_eq!(cfg.effective_thresholds("claude").warn_pct, 50.0);
        assert_eq!(cfg.effective_thresholds("codex").warn_pct, 80.0);
    }

    #[test]
    fn old_unversioned_config_keeps_default_account_identity() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.json"),
            r#"{"providers":{"claude":{"enabled":true}}}"#,
        )
        .unwrap();
        let cfg = Config::load(dir.path());
        assert_eq!(cfg.version, 1);
        assert_eq!(cfg.providers["claude"].kind, None);
        assert_eq!(cfg.providers["claude"].label, None);
    }

    #[test]
    fn saved_provider_order_and_new_metric_setting_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        let claude = cfg.providers.shift_remove("claude").unwrap();
        let codex = cfg.providers.shift_remove("codex").unwrap();
        cfg.providers.insert("codex".into(), codex);
        cfg.providers.insert("claude".into(), claude);
        cfg.providers["claude"].mini_summary_metric = Some("window:five_hour".into());
        cfg.save(dir.path()).unwrap();

        let loaded = Config::load(dir.path());
        assert_eq!(
            loaded.providers.keys().collect::<Vec<_>>(),
            vec![&"openrouter", &"hermes", &"codex", &"claude"]
        );
        assert_eq!(
            loaded.providers["claude"].mini_summary_metric.as_deref(),
            Some("window:five_hour")
        );
    }

    /// Saving from Settings does not go straight from struct to file: the
    /// config crosses IPC as a `serde_json::Value` first. Without serde_json's
    /// `preserve_order` feature that intermediate `Map` is a `BTreeMap`, which
    /// silently re-sorts the account keys alphabetically and threw away every
    /// reorder the user made. The file round-trip above cannot catch this
    /// because it never builds a `Value`.
    #[test]
    fn provider_order_survives_a_round_trip_through_serde_json_value() {
        let mut cfg = Config::default();
        let claude = cfg.providers.shift_remove("claude").unwrap();
        cfg.providers.insert("claude".into(), claude); // move claude last
        let before: Vec<String> = cfg.providers.keys().cloned().collect();
        assert_ne!(
            before,
            {
                let mut sorted = before.clone();
                sorted.sort();
                sorted
            },
            "test is only meaningful if the order is not already alphabetical"
        );

        let via_value = serde_json::to_value(&cfg).unwrap();
        let back: Config = serde_json::from_value(via_value).unwrap();

        assert_eq!(back.providers.keys().cloned().collect::<Vec<_>>(), before);
    }
}
