use crate::model::{Status, UsageSnapshot};
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
    /// Superseded by `mini_summary_metrics`. Kept so older files still load
    /// and so a downgrade finds the value it wrote; `migrate_mini_summary`
    /// folds it into the list on load.
    pub mini_summary_metric: Option<String>,
    /// Headlines shown for this account in the compact tray-click summary, one
    /// row each, in the order given. `None` preserves the automatic
    /// worst-window/credits selection; an empty list omits the account.
    pub mini_summary_metrics: Option<Vec<String>>,
    /// Which headline drives the tray icon's status and gauge. `None` folds
    /// the worst across every selected headline; `"none"` contributes nothing;
    /// otherwise the named metric alone.
    pub tray_metric: Option<String>,
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
            mini_summary_metrics: None,
            tray_metric: None,
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
    /// Let scrolling over a window fade its painted shell. The level itself is
    /// deliberately ephemeral so reopening the widget never leaves it hidden.
    pub scroll_opacity: bool,
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
        providers.insert("elevenlabs".into(), ProviderConfig::default());
        providers.insert("firecrawl".into(), ProviderConfig::default());
        providers.insert("deepseek".into(), ProviderConfig::default());
        providers.insert("hermes".into(), ProviderConfig::default());
        Self {
            version: 2,
            poll_interval_secs: 60,
            thresholds: Thresholds::default(),
            alerts: AlertToggles::default(),
            autostart: false,
            hide_on_blur: false,
            mini_summary_bars: true,
            scroll_opacity: true,
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

    /// The headlines to show for an account. `None` means "decide
    /// automatically" — the caller picks the worst gating window, or credits.
    /// An empty list means the account is deliberately omitted. Unknown
    /// providers (a config written by a newer build) fall back to automatic.
    pub fn resolved_mini_metrics(&self, provider_id: &str) -> Option<Vec<String>> {
        self.providers
            .get(provider_id)
            .and_then(|p| p.mini_summary_metrics.clone())
    }

    /// Which single headline drives the tray, if the user pinned one. `None`
    /// means fold across every selected headline.
    pub fn tray_metric(&self, provider_id: &str) -> Option<String> {
        self.providers
            .get(provider_id)
            .and_then(|p| p.tray_metric.clone())
    }

    /// The status and optional gauge percentage an account contributes to the
    /// tray icon. The user either pins one headline or lets the worst of the
    /// selected headlines win; `"none"` deliberately contributes nothing,
    /// which lets an account stay visible in the main popup without appearing
    /// in either compact surface.
    pub fn mini_tray_status(
        &self,
        snapshot: &UsageSnapshot,
        low_balance_warn: Option<f64>,
    ) -> Option<(Status, Option<f64>)> {
        let thresholds = self.effective_thresholds(&snapshot.provider_id);
        let (warn_pct, critical_pct) = (thresholds.warn_pct, thresholds.critical_pct);
        let pinned = self.tray_metric(&snapshot.provider_id);
        if pinned.as_deref() == Some("none") {
            return None;
        }
        if snapshot.error.is_some() {
            return Some((Status::Stale, None));
        }
        if let Some(metric) = pinned {
            return metric_tray_status(snapshot, &metric, warn_pct, critical_pct, low_balance_warn);
        }

        // Worst of the selected headlines. An empty selection is the account
        // opting out of the summary entirely, so it contributes nothing.
        if let Some(selected) = self.resolved_mini_metrics(&snapshot.provider_id) {
            if selected.is_empty() {
                return None;
            }
            return selected
                .iter()
                .filter_map(|metric| {
                    metric_tray_status(snapshot, metric, warn_pct, critical_pct, low_balance_warn)
                })
                .reduce(|(status_a, pct_a), (status_b, pct_b)| {
                    (status_a.max(status_b), max_pct(pct_a, pct_b))
                });
        }

        // Automatic: the worst real quota window, as before.
        let pct = snapshot
            .windows
            .iter()
            .filter(|window| !window.informational)
            .map(|window| window.used_pct)
            .max_by(f64::total_cmp);
        Some((
            snapshot.status(warn_pct, critical_pct, low_balance_warn),
            pct,
        ))
    }

    /// Folds the pre-v2 single-headline field into the list form. Gated on the
    /// file version rather than on the new fields being absent, because
    /// `tray_metric: None` is itself a meaningful value ("worst of selected")
    /// and so cannot double as "not yet migrated".
    fn migrate_mini_summary(&mut self) {
        if self.version >= 2 {
            return;
        }
        for provider in self.providers.values_mut() {
            provider.mini_summary_metrics = match provider.mini_summary_metric.as_deref() {
                // Automatic: leave as None so the adaptive pick continues.
                None => None,
                Some("none") => Some(Vec::new()),
                Some(metric) => Some(vec![metric.to_string()]),
            };
            // The old dropdown pinned the tray to exactly the one selected
            // metric, and the old checkbox was the only way to opt out.
            provider.tray_metric = if !provider.in_tray {
                Some("none".into())
            } else {
                provider.mini_summary_metric.clone().map(|metric| {
                    if metric == "none" {
                        "none".into()
                    } else {
                        metric
                    }
                })
            };
        }
        self.version = 2;
    }

    pub fn provider_setting(&self, provider_id: &str, key: &str) -> Option<serde_json::Value> {
        self.providers.get(provider_id)?.settings.get(key).cloned()
    }

    pub fn load(dir: &Path) -> Self {
        let path = dir.join("config.json");
        let mut cfg: Self = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        };
        cfg.migrate_mini_summary();
        cfg
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let mut out = self.clone();
        out.migrate_mini_summary();
        // The pre-v2 fields are dead to this build but a downgrade still reads
        // them, so mirror the picker into them: the first selected headline is
        // the closest single-value equivalent.
        for provider in out.providers.values_mut() {
            provider.in_tray = provider.tray_metric.as_deref() != Some("none");
            provider.mini_summary_metric = match provider.mini_summary_metrics.as_deref() {
                None => None,
                Some([]) => Some("none".into()),
                Some([first, ..]) => Some(first.clone()),
            };
        }
        let text = serde_json::to_string_pretty(&out).expect("config serializes");
        let path = dir.join("config.json");
        let tmp = dir.join("config.json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }
}

/// One selected headline's contribution to the tray. A window no longer in the
/// snapshot — a provider dropped it, or the account was reconfigured — yields
/// nothing rather than a misleading calm 0%.
fn metric_tray_status(
    snapshot: &UsageSnapshot,
    metric: &str,
    warn_pct: f64,
    critical_pct: f64,
    low_balance_warn: Option<f64>,
) -> Option<(Status, Option<f64>)> {
    if let Some(metric_id) = metric.strip_prefix("window:") {
        let window = snapshot
            .windows
            .iter()
            .find(|window| window.metric_id == metric_id)?;
        let status = if window.used_pct >= critical_pct {
            Status::Critical
        } else if window.used_pct >= warn_pct {
            Status::Warn
        } else {
            Status::Ok
        };
        return Some((status, Some(window.used_pct)));
    }
    if metric == "credits" {
        return Some((
            snapshot.status(warn_pct, critical_pct, low_balance_warn),
            None,
        ));
    }
    None
}

fn max_pct(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (some, None) | (None, some) => some,
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
        // Load migrates an unversioned file forward.
        assert_eq!(cfg.version, 2);
        assert_eq!(cfg.providers["claude"].kind, None);
        assert_eq!(cfg.providers["claude"].label, None);
        assert!(cfg.scroll_opacity);
    }

    /// Every pre-v2 headline setting has to land on the equivalent list, or an
    /// upgrade silently changes what the mini summary and tray show.
    #[test]
    fn single_headline_setting_migrates_to_the_list_form() {
        let cases = [
            // (mini_summary_metric, in_tray, expected metrics, expected tray)
            (None, true, None, None),
            (Some("none"), true, Some(vec![]), Some("none")),
            (
                Some("window:five_hour"),
                true,
                Some(vec!["window:five_hour".to_string()]),
                Some("window:five_hour"),
            ),
            // The opt-out checkbox wins over whichever headline was picked.
            (
                Some("credits"),
                false,
                Some(vec!["credits".to_string()]),
                Some("none"),
            ),
            (None, false, None, Some("none")),
        ];
        for (metric, in_tray, want_metrics, want_tray) in cases {
            let dir = tempfile::tempdir().unwrap();
            let metric_json = match metric {
                Some(m) => format!(r#""{m}""#),
                None => "null".into(),
            };
            std::fs::write(
                dir.path().join("config.json"),
                format!(
                    r#"{{"version":1,"providers":{{"claude":{{"enabled":true,"in_tray":{in_tray},"mini_summary_metric":{metric_json}}}}}}}"#
                ),
            )
            .unwrap();
            let cfg = Config::load(dir.path());
            let claude = &cfg.providers["claude"];
            assert_eq!(
                claude.mini_summary_metrics, want_metrics,
                "metrics {metric:?}/{in_tray}"
            );
            assert_eq!(
                claude.tray_metric.as_deref(),
                want_tray,
                "tray {metric:?}/{in_tray}"
            );
        }
    }

    /// A v2 file is authoritative: re-running the migration over it would
    /// overwrite a multi-metric selection with the single-value mirror that
    /// `save` writes for downgrades.
    #[test]
    fn migration_does_not_rewrite_an_already_migrated_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.providers["claude"].mini_summary_metrics =
            Some(vec!["window:five_hour".into(), "window:weekly".into()]);
        cfg.providers["claude"].tray_metric = Some("window:weekly".into());
        cfg.save(dir.path()).unwrap();

        let loaded = Config::load(dir.path());
        assert_eq!(
            loaded.providers["claude"].mini_summary_metrics,
            Some(vec!["window:five_hour".into(), "window:weekly".into()])
        );
        assert_eq!(
            loaded.providers["claude"].tray_metric.as_deref(),
            Some("window:weekly")
        );
        // The downgrade mirror is the first selected headline.
        assert_eq!(
            loaded.providers["claude"].mini_summary_metric.as_deref(),
            Some("window:five_hour")
        );
    }

    #[test]
    fn saved_provider_order_and_new_metric_setting_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        let claude = cfg.providers.shift_remove("claude").unwrap();
        let codex = cfg.providers.shift_remove("codex").unwrap();
        cfg.providers.insert("codex".into(), codex);
        cfg.providers.insert("claude".into(), claude);
        cfg.providers["claude"].mini_summary_metrics = Some(vec!["window:five_hour".into()]);
        cfg.save(dir.path()).unwrap();

        let loaded = Config::load(dir.path());
        assert_eq!(
            loaded.providers.keys().collect::<Vec<_>>(),
            vec![
                &"openrouter",
                &"elevenlabs",
                &"firecrawl",
                &"deepseek",
                &"hermes",
                &"codex",
                &"claude"
            ]
        );
        assert_eq!(
            loaded.providers["claude"].mini_summary_metrics,
            Some(vec!["window:five_hour".to_string()])
        );
    }

    mod tray {
        use super::*;
        use crate::model::UsageWindow;

        fn snapshot() -> UsageSnapshot {
            UsageSnapshot::ok(
                "claude",
                "Claude",
                vec![
                    UsageWindow {
                        metric_id: "five_hour".into(),
                        label: "5-hour".into(),
                        used_pct: 13.0,
                        ..Default::default()
                    },
                    UsageWindow {
                        metric_id: "weekly".into(),
                        label: "Weekly".into(),
                        used_pct: 88.0,
                        ..Default::default()
                    },
                ],
                None,
            )
        }

        fn status_of(
            metrics: Option<Vec<&str>>,
            tray: Option<&str>,
        ) -> Option<(Status, Option<f64>)> {
            let mut cfg = Config::default();
            let claude = cfg.providers.get_mut("claude").unwrap();
            claude.mini_summary_metrics =
                metrics.map(|m| m.into_iter().map(String::from).collect());
            claude.tray_metric = tray.map(String::from);
            cfg.mini_tray_status(&snapshot(), None)
        }

        /// The headline the user pinned is the only one that counts, even when
        /// a louder one is also on screen.
        #[test]
        fn a_pinned_metric_drives_the_tray_alone() {
            let got = status_of(
                Some(vec!["window:five_hour", "window:weekly"]),
                Some("window:five_hour"),
            );
            assert_eq!(got, Some((Status::Ok, Some(13.0))));
        }

        /// Without a pin, no selected headline may go critical unnoticed.
        #[test]
        fn worst_of_selected_folds_across_every_headline() {
            let got = status_of(Some(vec!["window:five_hour", "window:weekly"]), None);
            assert_eq!(got, Some((Status::Warn, Some(88.0))));
        }

        #[test]
        fn none_and_an_empty_selection_contribute_nothing() {
            assert_eq!(status_of(Some(vec![]), None), None);
            assert_eq!(status_of(Some(vec!["window:weekly"]), Some("none")), None);
        }

        /// Automatic still means "worst real quota window".
        #[test]
        fn automatic_selection_keeps_the_old_behaviour() {
            assert_eq!(status_of(None, None), Some((Status::Warn, Some(88.0))));
        }

        /// A selection naming a window the provider stopped reporting must not
        /// register as a calm 0%.
        #[test]
        fn a_missing_window_is_skipped_rather_than_counted_as_zero() {
            let got = status_of(Some(vec!["window:gone", "window:five_hour"]), None);
            assert_eq!(got, Some((Status::Ok, Some(13.0))));
            assert_eq!(status_of(Some(vec!["window:gone"]), None), None);
        }
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
