//! Pure credential-transfer logic: build a bundle from the shared configuration
//! plus a secret reader, and apply that bundle onto a target's shared
//! configuration and secret store.
//!
//! This module deliberately knows nothing about cryptography, sockets, QR
//! codes, or platform keychains. It is the single seam every host and transport
//! calls into, matching how `crate::refresh` is the shared operation every host
//! calls for quota fetching.
//!
//! See `docs/adr/0008-credential-sync-is-direct-e2e-and-never-copies-oauth.md`
//! for the design rationale: OAuth and cookie accounts move as *shells* and
//! re-authenticate on the destination, so rotating sessions are never copied
//! between devices.

use crate::config::ProviderConfig;
use crate::shared_config::SharedConfig;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Providers whose credential is an OAuth session or a cookie rather than a
/// stable pasted API key. These accounts move as shells: the destination keeps
/// the account entry but must sign in again to obtain its own session.
const SHELL_KINDS: &[&str] = &["claude", "codex", "grok", "hermes"];

fn is_shell_kind(kind: &str) -> bool {
    SHELL_KINDS.contains(&kind)
}

/// A portable snapshot of one account, ready to be sealed and moved to another
/// device.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BundleAccount {
    /// The account entry exactly as it appears in the source's shared
    /// configuration. Account keys are immutable identity, so a relabel on the
    /// source updates the same account on the destination rather than
    /// duplicating it.
    pub config: ProviderConfig,
    /// The pasted API key for direct-key providers. `None` for OAuth/cookie
    /// shells and for any pasted-key account that has no key stored yet.
    pub secret: Option<String>,
}

/// Credential bundle: every account entry plus, for pasted-key providers, its
/// API key. The bundle is the plaintext payload that sealing, pairing and
/// export all move around.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CredentialBundle {
    /// Format version for forward compatibility. An unknown version is refused
    /// by the open path rather than mis-parsed.
    #[serde(default = "bundle_version_default")]
    pub version: u32,
    /// Accounts in their source order. The apply path preserves this order for
    /// newly added accounts and updates existing accounts in place.
    pub accounts: IndexMap<String, BundleAccount>,
}

fn bundle_version_default() -> u32 {
    1
}

impl Default for CredentialBundle {
    fn default() -> Self {
        Self {
            version: bundle_version_default(),
            accounts: IndexMap::new(),
        }
    }
}

/// What happened to one account during [`apply_bundle`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum ApplyOutcome {
    /// The account did not exist on the destination and was added.
    Added,
    /// The account already existed and was updated in place.
    Updated,
    /// The account is an OAuth/cookie shell and needs a fresh sign-in on the
    /// destination.
    NeedsOnboarding,
    /// The account's API key could not be stored (e.g. a keystore failure).
    /// The account is left untouched so it does not appear set up when it is
    /// not.
    CouldNotStore { reason: String },
}

/// The result of applying a bundle to a target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplyReport {
    /// One outcome per account, in the order the bundle presented them.
    pub accounts: IndexMap<String, ApplyOutcome>,
}

/// Build a [`CredentialBundle`] from the shared configuration, reading each
/// pasted-key provider's secret through `read_secret`. OAuth/cookie providers
/// are included as shells with no secret.
///
/// `read_secret` is called with the account's immutable config key. For a
/// pasted-key account the secret lives under that exact key; for OAuth/cookie
/// accounts the live token lives under a different name (`claude_oauth`, etc.)
/// and is deliberately not read.
pub fn build_bundle(
    shared: &SharedConfig,
    mut read_secret: impl FnMut(&str) -> Option<String>,
) -> CredentialBundle {
    let mut accounts = IndexMap::with_capacity(shared.providers.len());
    for (key, config) in &shared.providers {
        let kind = config.kind.as_deref().unwrap_or(key);
        let secret = if is_shell_kind(kind) {
            None
        } else {
            read_secret(key)
        };
        accounts.insert(
            key.clone(),
            BundleAccount {
                config: config.clone(),
                secret,
            },
        );
    }
    CredentialBundle {
        version: bundle_version_default(),
        accounts,
    }
}

/// Insert or update one account entry and return whether it was added or
/// updated. Existing accounts are matched by immutable config key and updated
/// in place so their order is preserved; new accounts are appended.
fn insert_config(target: &mut SharedConfig, key: &str, config: &ProviderConfig) -> ApplyOutcome {
    let existed = target.providers.contains_key(key);
    target.providers.insert(key.into(), config.clone());
    if existed {
        ApplyOutcome::Updated
    } else {
        ApplyOutcome::Added
    }
}

/// Apply a bundle to a target shared configuration, writing pasted-key secrets
/// through `write_secret`. Existing accounts are matched by immutable config
/// key and updated in place; new accounts are appended in bundle order.
///
/// OAuth/cookie shells have no secret to move, so the account entry is written
/// and the report always marks it as [`ApplyOutcome::NeedsOnboarding`] — the
/// destination must sign in again to obtain its own session.
///
/// If a pasted-key secret cannot be stored, the account entry is *not* written
/// and the report marks it as [`ApplyOutcome::CouldNotStore`] so the
/// destination does not appear to have a working account.
pub fn apply_bundle(
    bundle: &CredentialBundle,
    target: &mut SharedConfig,
    mut write_secret: impl FnMut(&str, &str) -> Result<(), String>,
) -> ApplyReport {
    let mut report = IndexMap::with_capacity(bundle.accounts.len());
    for (key, account) in &bundle.accounts {
        let kind = account.config.kind.as_deref().unwrap_or(key);
        let outcome = if is_shell_kind(kind) {
            insert_config(target, key, &account.config);
            ApplyOutcome::NeedsOnboarding
        } else if let Some(secret) = &account.secret {
            match write_secret(key, secret) {
                Ok(()) => insert_config(target, key, &account.config),
                Err(reason) => ApplyOutcome::CouldNotStore { reason },
            }
        } else {
            // Pasted-key account with no key in the bundle: move the entry so
            // the account is visible, but there is nothing to store.
            insert_config(target, key, &account.config)
        };
        report.insert(key.clone(), outcome);
    }
    ApplyReport { accounts: report }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pasted_config(enabled: bool) -> ProviderConfig {
        ProviderConfig {
            enabled,
            kind: Some("openrouter".into()),
            label: None,
            ..ProviderConfig::default()
        }
    }

    fn oauth_config(kind: &str, enabled: bool) -> ProviderConfig {
        ProviderConfig {
            enabled,
            kind: Some(kind.into()),
            label: None,
            ..ProviderConfig::default()
        }
    }

    #[test]
    fn build_bundle_includes_pasted_key_and_omits_oauth_secret() {
        let mut shared = SharedConfig::default();
        shared.providers.clear();
        shared
            .providers
            .insert("openrouter".into(), pasted_config(true));
        shared
            .providers
            .insert("claude".into(), oauth_config("claude", true));

        let mut secrets: IndexMap<String, String> = IndexMap::new();
        secrets.insert("openrouter".into(), "sk-or-v1-abc".into());
        secrets.insert("claude_oauth".into(), "secret-oauth-token".into());

        let bundle = build_bundle(&shared, |key| secrets.get(key).cloned());

        assert_eq!(bundle.accounts.len(), 2);
        assert_eq!(
            bundle.accounts["openrouter"].secret.as_deref(),
            Some("sk-or-v1-abc")
        );
        assert!(bundle.accounts["claude"].secret.is_none());
        assert_eq!(
            bundle.accounts["claude"].config.kind.as_deref(),
            Some("claude")
        );
    }

    #[test]
    fn build_bundle_treats_hermes_as_a_shell() {
        let mut shared = SharedConfig::default();
        shared.providers.clear();
        shared
            .providers
            .insert("hermes".into(), oauth_config("hermes", true));

        let mut secrets: IndexMap<String, String> = IndexMap::new();
        secrets.insert("hermes".into(), "portal-session-cookie".into());

        let bundle = build_bundle(&shared, |key| secrets.get(key).cloned());

        assert!(bundle.accounts["hermes"].secret.is_none());
    }

    #[test]
    fn apply_bundle_adds_every_account_to_empty_target_and_writes_keys() {
        let mut shared = SharedConfig::default();
        shared.providers.clear();
        shared
            .providers
            .insert("openrouter".into(), pasted_config(true));
        shared
            .providers
            .insert("claude".into(), oauth_config("claude", true));

        let secrets: IndexMap<String, String> = IndexMap::from([
            ("openrouter".into(), "sk-or-v1-abc".into()),
            ("claude_oauth".into(), "token".into()),
        ]);
        let bundle = build_bundle(&shared, |key| secrets.get(key).cloned());

        let mut target = SharedConfig::default();
        target.providers.clear();
        let mut stored: IndexMap<String, String> = IndexMap::new();
        let report = apply_bundle(&bundle, &mut target, |key, value| {
            stored.insert(key.into(), value.into());
            Ok(())
        });

        assert_eq!(target.providers.len(), 2);
        assert!(target.providers.contains_key("openrouter"));
        assert!(target.providers.contains_key("claude"));
        assert_eq!(
            stored.get("openrouter").map(String::as_str),
            Some("sk-or-v1-abc")
        );
        assert!(!stored.contains_key("claude"));

        assert_eq!(report.accounts["openrouter"], ApplyOutcome::Added);
        assert_eq!(report.accounts["claude"], ApplyOutcome::NeedsOnboarding);
    }

    #[test]
    fn apply_bundle_updates_existing_account_by_key_not_label() {
        let mut shared = SharedConfig::default();
        shared.providers.clear();
        shared.providers.insert(
            "claude#work".into(),
            ProviderConfig {
                enabled: true,
                kind: Some("claude".into()),
                label: Some("Work Claude".into()),
                ..ProviderConfig::default()
            },
        );

        let bundle = build_bundle(&shared, |_key| None);

        let mut target = SharedConfig::default();
        target.providers.clear();
        target.providers.insert(
            "claude#work".into(),
            ProviderConfig {
                enabled: false,
                kind: Some("claude".into()),
                label: Some("Old Label".into()),
                ..ProviderConfig::default()
            },
        );

        let report = apply_bundle(&bundle, &mut target, |_key, _value| unreachable!());

        assert_eq!(target.providers.len(), 1);
        assert_eq!(
            target.providers["claude#work"].label.as_deref(),
            Some("Work Claude")
        );
        assert!(target.providers["claude#work"].enabled);
        assert_eq!(
            report.accounts["claude#work"],
            ApplyOutcome::NeedsOnboarding
        );
    }

    #[test]
    fn apply_bundle_does_not_duplicate_a_relabelled_account() {
        let mut shared = SharedConfig::default();
        shared.providers.clear();
        shared.providers.insert(
            "openrouter".into(),
            ProviderConfig {
                enabled: true,
                kind: Some("openrouter".into()),
                label: Some("Renamed".into()),
                ..ProviderConfig::default()
            },
        );

        let secrets: IndexMap<String, String> =
            IndexMap::from([("openrouter".into(), "sk-or-v1-abc".into())]);
        let bundle = build_bundle(&shared, |key| secrets.get(key).cloned());

        let mut target = SharedConfig::default();
        target.providers.clear();
        target.providers.insert(
            "openrouter".into(),
            ProviderConfig {
                enabled: true,
                kind: Some("openrouter".into()),
                label: Some("Original".into()),
                ..ProviderConfig::default()
            },
        );

        let report = apply_bundle(&bundle, &mut target, |_key, _value| Ok(()));

        assert_eq!(target.providers.len(), 1);
        assert_eq!(
            target.providers["openrouter"].label.as_deref(),
            Some("Renamed")
        );
        assert_eq!(report.accounts["openrouter"], ApplyOutcome::Updated);
    }

    #[test]
    fn apply_bundle_reports_could_not_store_and_leaves_account_untouched() {
        let mut shared = SharedConfig::default();
        shared.providers.clear();
        shared
            .providers
            .insert("openrouter".into(), pasted_config(true));

        let secrets: IndexMap<String, String> =
            IndexMap::from([("openrouter".into(), "sk-or-v1-abc".into())]);
        let bundle = build_bundle(&shared, |key| secrets.get(key).cloned());

        let mut target = SharedConfig::default();
        target.providers.clear();

        let report = apply_bundle(&bundle, &mut target, |_key, _value| {
            Err("keystore locked".into())
        });

        assert!(target.providers.is_empty());
        assert_eq!(
            report.accounts["openrouter"],
            ApplyOutcome::CouldNotStore {
                reason: "keystore locked".into()
            }
        );
    }

    #[test]
    fn apply_bundle_preserves_existing_accounts_not_in_bundle() {
        let mut shared = SharedConfig::default();
        shared.providers.clear();
        shared
            .providers
            .insert("openrouter".into(), pasted_config(true));

        let secrets: IndexMap<String, String> =
            IndexMap::from([("openrouter".into(), "sk-or-v1-abc".into())]);
        let bundle = build_bundle(&shared, |key| secrets.get(key).cloned());

        let mut target = SharedConfig::default();
        target.providers.clear();
        target.providers.insert(
            "elevenlabs".into(),
            ProviderConfig {
                enabled: true,
                kind: Some("elevenlabs".into()),
                ..ProviderConfig::default()
            },
        );

        apply_bundle(&bundle, &mut target, |_key, _value| Ok(()));

        assert_eq!(target.providers.len(), 2);
        assert!(target.providers.contains_key("openrouter"));
        assert!(target.providers.contains_key("elevenlabs"));
    }

    #[test]
    fn apply_bundle_keeps_existing_account_when_secret_write_fails() {
        let mut shared = SharedConfig::default();
        shared.providers.clear();
        shared
            .providers
            .insert("openrouter".into(), pasted_config(true));

        let secrets: IndexMap<String, String> =
            IndexMap::from([("openrouter".into(), "new-key".into())]);
        let bundle = build_bundle(&shared, |key| secrets.get(key).cloned());

        let mut target = SharedConfig::default();
        target.providers.clear();
        target.providers.insert(
            "openrouter".into(),
            ProviderConfig {
                enabled: true,
                kind: Some("openrouter".into()),
                label: Some("Original".into()),
                ..ProviderConfig::default()
            },
        );

        let report = apply_bundle(&bundle, &mut target, |_key, _value| {
            Err("keystore locked".into())
        });

        assert_eq!(target.providers.len(), 1);
        assert_eq!(
            target.providers["openrouter"].label.as_deref(),
            Some("Original")
        );
        assert_eq!(
            report.accounts["openrouter"],
            ApplyOutcome::CouldNotStore {
                reason: "keystore locked".into()
            }
        );
    }

    #[test]
    fn bundle_serializes_and_deserializes() {
        let mut bundle = CredentialBundle::default();
        bundle.accounts.insert(
            "openrouter".into(),
            BundleAccount {
                config: pasted_config(true),
                secret: Some("sk-or-v1-abc".into()),
            },
        );

        let json = serde_json::to_string(&bundle).unwrap();
        let round: CredentialBundle = serde_json::from_str(&json).unwrap();

        assert_eq!(round, bundle);
        assert_eq!(round.version, 1);
    }
}
