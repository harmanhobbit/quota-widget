use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Thresholds {
    pub warn_pct: f64,
    pub critical_pct: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self { warn_pct: 80.0, critical_pct: 95.0 }
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
        Self { toast: true, tray_color: true, auto_popup: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ProviderConfig {
    pub enabled: bool,
    /// Overrides the global thresholds when set.
    pub thresholds: Option<Thresholds>,
    /// Overrides the global alert toggles when set.
    pub alerts: Option<AlertToggles>,
    /// Warn when a credit balance drops to/below this value.
    pub low_balance_warn: Option<f64>,
    /// Provider-specific knobs (endpoint overrides, token price, …).
    pub settings: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub poll_interval_secs: u64,
    pub thresholds: Thresholds,
    pub alerts: AlertToggles,
    pub autostart: bool,
    pub providers: BTreeMap<String, ProviderConfig>,
}

impl Default for Config {
    fn default() -> Self {
        let mut providers = BTreeMap::new();
        // Claude and Codex work out of the box when their CLIs are logged in.
        providers.insert("claude".into(), ProviderConfig { enabled: true, ..Default::default() });
        providers.insert("codex".into(), ProviderConfig { enabled: true, ..Default::default() });
        providers.insert("openrouter".into(), ProviderConfig::default());
        providers.insert("hermes".into(), ProviderConfig::default());
        Self {
            poll_interval_secs: 60,
            thresholds: Thresholds::default(),
            alerts: AlertToggles::default(),
            autostart: false,
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
        std::fs::write(dir.join("config.json"), text)
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
        cfg.providers.get_mut("claude").unwrap().thresholds =
            Some(Thresholds { warn_pct: 50.0, critical_pct: 75.0 });
        assert_eq!(cfg.effective_thresholds("claude").warn_pct, 50.0);
        assert_eq!(cfg.effective_thresholds("codex").warn_pct, 80.0);
    }
}
