//! Shared configuration: the accounts, provider settings, thresholds, alerts,
//! ordering and headline selections whose meaning is common to every Quota
//! Widget platform. See CONTEXT.md "Shared configuration" and
//! docs/adr/0006-share-the-domain-and-foreground-ui-with-a-native-android-host.md.
//!
//! This is the user-authored half of what `crate::config::Config` used to
//! hold in one file. Its corruption policy is a direct port of `Config`'s: the
//! accounts and provider settings here took real effort to enter, and the
//! secret-store keys derived from provider keys (see AGENTS.md, "Secret keys
//! are derived from config") mean a file we cannot parse still names secrets
//! we cannot enumerate. So a file that exists but cannot be read or parsed is
//! kept exactly where it is, running on defaults, until the user decides what
//! happens to it — never silently replaced. Contrast
//! `crate::platform_preferences::PlatformPreferences`, whose loss costs a few
//! re-clicked toggles and which is therefore freely replaceable.

use crate::config::{AlertToggles, Config, ProviderConfig, SortBasis, SortOrder, Thresholds};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const FILE_NAME: &str = "shared-config.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SharedConfig {
    /// Reserved for future migrations. A file written before the split
    /// deserializes as 0 as unversioned legacy `config.json` files always
    /// have.
    pub version: u32,
    pub thresholds: Thresholds,
    pub alerts: AlertToggles,
    pub sort_order: SortOrder,
    pub sort_basis: SortBasis,
    /// Account iteration order is the user-selected display order everywhere.
    /// Account keys are immutable identity: relabelling an account never
    /// changes its map key, which is also what the secret store and every
    /// per-account override (thresholds, alerts, headline picks) are keyed
    /// on.
    pub providers: IndexMap<String, ProviderConfig>,
}

impl Default for SharedConfig {
    fn default() -> Self {
        Self {
            version: 1,
            thresholds: Thresholds::default(),
            alerts: AlertToggles::default(),
            sort_order: SortOrder::default(),
            sort_basis: SortBasis::default(),
            providers: Config::default().providers,
        }
    }
}

impl SharedConfig {
    /// The fields carried over from a pre-split `config.json`. Account keys,
    /// labels and every per-provider override are copied verbatim — including
    /// entries for a provider `kind` this build does not recognise, which
    /// `providers_for` already tolerates by skipping instantiation while
    /// leaving the data alone (see `providers::providers_for`).
    pub fn from_legacy(config: &Config) -> Self {
        Self {
            version: 1,
            thresholds: config.thresholds.clone(),
            alerts: config.alerts.clone(),
            sort_order: config.sort_order,
            sort_basis: config.sort_basis,
            providers: config.providers.clone(),
        }
    }

    /// Reads `shared-config.json`. Three outcomes, exactly mirroring
    /// `Config::load`: no file is a first run on defaults; a valid file loads
    /// as-is; a file that exists but cannot be read or parsed runs on
    /// defaults *and* reports a [`SharedConfigRecovery`].
    pub fn load(dir: &Path) -> SharedConfigLoad {
        let path = dir.join(FILE_NAME);
        let (config, recovery) = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Self>(&text) {
                Ok(cfg) => (cfg, None),
                Err(e) => (
                    Self::default(),
                    Some(SharedConfigRecovery {
                        kind: RecoveryKind::Malformed,
                        path,
                        detail: e.to_string(),
                    }),
                ),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (
                Self::default(),
                Some(SharedConfigRecovery {
                    kind: RecoveryKind::Unreadable,
                    path,
                    detail: e.to_string(),
                }),
            ),
        };
        SharedConfigLoad { config, recovery }
    }

    pub fn recovery_state(dir: &Path) -> Option<SharedConfigRecovery> {
        Self::load(dir).recovery
    }

    /// Writes the config, unless doing so would destroy an existing one we
    /// could not read. See [`Config::save`], which this mirrors.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        if let Some(recovery) = Self::recovery_state(dir) {
            return Err(refuse_overwrite(&recovery));
        }
        self.write(dir)
    }

    /// The explicit recovery action: move the unreadable original aside, then
    /// save. See [`Config::save_after_recovery`], which this mirrors.
    pub fn save_after_recovery(&self, dir: &Path) -> std::io::Result<Option<PathBuf>> {
        let Some(recovery) = Self::recovery_state(dir) else {
            self.write(dir)?;
            return Ok(None);
        };
        let kept = free_backup_path(dir);
        std::fs::rename(&recovery.path, &kept)?;
        self.write(dir)?;
        Ok(Some(kept))
    }

    fn write(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let text = serde_json::to_string_pretty(self).expect("shared config serializes");
        let path = dir.join(FILE_NAME);
        let tmp = dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }
}

/// Why an existing `shared-config.json` could not be turned into a
/// [`SharedConfig`]. Identical shape to [`crate::config::RecoveryKind`],
/// kept as its own type so this module has no dependency on the legacy
/// combined-file recovery type surviving past its eventual removal.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryKind {
    Malformed,
    Unreadable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedConfigRecovery {
    pub kind: RecoveryKind,
    pub path: PathBuf,
    pub detail: String,
}

impl SharedConfigRecovery {
    pub fn message(&self) -> String {
        let what = match self.kind {
            RecoveryKind::Malformed => "could not be parsed",
            RecoveryKind::Unreadable => "could not be read",
        };
        format!(
            "{} {what} ({}). Running on defaults; the original has been kept and \
             saving is blocked until it is recovered or replaced.",
            self.path.display(),
            self.detail
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SharedConfigLoad {
    pub config: SharedConfig,
    pub recovery: Option<SharedConfigRecovery>,
}

fn refuse_overwrite(recovery: &SharedConfigRecovery) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "refusing to overwrite the existing shared configuration: {}",
            recovery.message()
        ),
    )
}

/// Where an unreadable shared config is kept when the user replaces it.
/// Numbered rather than overwritten — see `crate::config::free_backup_path`,
/// which this mirrors.
fn free_backup_path(dir: &Path) -> PathBuf {
    let first = dir.join("shared-config.json.unreadable");
    if !first.exists() {
        return first;
    }
    for n in 2u32.. {
        let candidate = dir.join(format!("shared-config.json.unreadable.{n}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("u32 exhausted while naming a backup")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(dir: &Path) -> SharedConfig {
        SharedConfig::load(dir).config
    }

    #[test]
    fn round_trip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = SharedConfig {
            sort_order: SortOrder::UsageDesc,
            ..Default::default()
        };
        cfg.providers.get_mut("openrouter").unwrap().enabled = true;
        cfg.save(dir.path()).unwrap();
        assert_eq!(load(dir.path()), cfg);
    }

    #[test]
    fn a_missing_file_is_a_first_run_on_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = SharedConfig::load(dir.path());
        assert_eq!(loaded.config, SharedConfig::default());
        assert_eq!(loaded.recovery, None);
        loaded.config.save(dir.path()).unwrap();
        assert_eq!(load(dir.path()), SharedConfig::default());
    }

    #[test]
    fn a_malformed_file_runs_on_defaults_and_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "{not json").unwrap();

        let loaded = SharedConfig::load(dir.path());
        assert_eq!(loaded.config, SharedConfig::default());
        let recovery = loaded.recovery.expect("malformed file reports recovery");
        assert_eq!(recovery.kind, RecoveryKind::Malformed);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
    }

    /// The core corruption-policy assertion: an ordinary save must refuse to
    /// replace an unreadable shared config, exactly as `Config::save` does —
    /// this is the "user-authored configuration blocks overwrite" half of the
    /// ticket's corruption policy.
    #[test]
    fn an_ordinary_save_refuses_to_replace_an_unreadable_shared_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        std::fs::write(&path, "{not json").unwrap();

        let err = SharedConfig::default()
            .save(dir.path())
            .expect_err("save must refuse");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{not json");
    }

    #[test]
    fn recovery_keeps_the_original_and_lets_the_next_save_through() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), "{not json").unwrap();

        let cfg = SharedConfig {
            sort_order: SortOrder::ExpirySoonest,
            ..Default::default()
        };
        let kept = cfg
            .save_after_recovery(dir.path())
            .unwrap()
            .expect("the original is kept somewhere");
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "{not json");

        let loaded = SharedConfig::load(dir.path());
        assert_eq!(loaded.recovery, None);
        assert_eq!(loaded.config.sort_order, SortOrder::ExpirySoonest);
    }

    /// Migration must preserve account identity: relabelling never changes
    /// which map key an account lives under, and this must survive the
    /// legacy → shared split untouched.
    #[test]
    fn migrating_preserves_immutable_account_keys_across_a_label_change() {
        let mut legacy = Config::default();
        legacy.providers.insert(
            "claude#work".into(),
            ProviderConfig {
                enabled: true,
                kind: Some("claude".into()),
                label: Some("Old Label".into()),
                ..Default::default()
            },
        );
        let shared = SharedConfig::from_legacy(&legacy);
        assert!(shared.providers.contains_key("claude#work"));
        assert_eq!(
            shared.providers["claude#work"].label.as_deref(),
            Some("Old Label")
        );

        // Relabelling changes the label, never the key.
        let mut relabelled = legacy.clone();
        relabelled.providers.get_mut("claude#work").unwrap().label = Some("New Label".into());
        let shared_after = SharedConfig::from_legacy(&relabelled);
        assert!(shared_after.providers.contains_key("claude#work"));
        assert_eq!(
            shared_after.providers["claude#work"].label.as_deref(),
            Some("New Label")
        );
    }

    /// A config.json written by a newer build can name a provider `kind` this
    /// build has never heard of. Migration must not drop that account merely
    /// because it cannot be instantiated — the user's settings and secrets for
    /// it are still real, and a downgrade or a next-build upgrade needs them
    /// intact.
    #[test]
    fn migrating_preserves_an_unknown_provider_kind_verbatim() {
        let mut legacy = Config::default();
        let mut settings = serde_json::Map::new();
        settings.insert("future_knob".into(), serde_json::json!(42));
        legacy.providers.insert(
            "mystery#1".into(),
            ProviderConfig {
                enabled: true,
                kind: Some("provider_from_the_future".into()),
                settings,
                ..Default::default()
            },
        );
        let shared = SharedConfig::from_legacy(&legacy);
        let entry = &shared.providers["mystery#1"];
        assert_eq!(entry.kind.as_deref(), Some("provider_from_the_future"));
        assert_eq!(entry.settings["future_knob"], serde_json::json!(42));

        // And it survives a further save/load round trip through the new
        // shared-config file untouched.
        let dir = tempfile::tempdir().unwrap();
        shared.save(dir.path()).unwrap();
        let reloaded = load(dir.path());
        assert_eq!(reloaded.providers["mystery#1"], entry.clone());
    }

    /// The full legacy → shared migration, exercised the way a real upgrade
    /// hits it: a `config.json` with hand-arranged provider order, overrides,
    /// and non-default sort settings, split into just its shared half.
    #[test]
    fn migration_carries_every_shared_field_and_nothing_else() {
        let mut legacy = Config {
            sort_order: SortOrder::UsageAsc,
            sort_basis: SortBasis::WorstCase,
            poll_interval_secs: 999, // platform-only; must not appear here
            autostart: true,         // platform-only; must not appear here
            ..Default::default()
        };
        legacy.thresholds.warn_pct = 50.0;
        legacy.providers.get_mut("claude").unwrap().thresholds = Some(Thresholds {
            warn_pct: 10.0,
            critical_pct: 20.0,
        });

        let shared = SharedConfig::from_legacy(&legacy);
        assert_eq!(shared.sort_order, SortOrder::UsageAsc);
        assert_eq!(shared.sort_basis, SortBasis::WorstCase);
        assert_eq!(shared.thresholds.warn_pct, 50.0);
        assert_eq!(
            shared.providers["claude"]
                .thresholds
                .as_ref()
                .unwrap()
                .warn_pct,
            10.0
        );
    }
}
