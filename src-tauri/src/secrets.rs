//! Secret storage for pasted API keys / cookies.
//!
//! On Windows the Credential Manager is used (via the `keyring` crate), so the
//! portable EXE leaves no plaintext secrets on disk. On Android, an app-only
//! Android Keystore key is used instead (also via `keyring`, a different
//! major version — see the `target_os = "android"` `backend` module below).
//! Elsewhere (Linux dev runs) secrets fall back to an owner-only, atomically
//! replaced JSON file in the config dir — `quota_core::secret_store`, which is
//! where the permission and atomicity rules and their tests live.

use quota_core::config::Config;
use std::collections::HashMap;
use std::path::Path;

#[cfg(not(mobile))]
pub use quota_core::secret_store::oauth_key;
/// Entry naming is shared with the file store, so the two backends agree on
/// which names a configuration implies. `oauth_key` derives the secret name
/// for a built-in sign-in flow (Claude/Codex/Grok OAuth), which is
/// desktop-only for now — every provider Android exposes (issue #109) is a
/// direct-HTTPS pasted key, not OAuth — so it is re-exported only where
/// desktop's `lib.rs` uses it.
pub use quota_core::secret_store::valid_key;

/// Windows Credential Manager caps one blob at CRED_MAX_CREDENTIAL_BLOB_SIZE
/// (2560 bytes, counted as UTF-16 — so 1280 code units). A Codex OAuth secret
/// is over that on its own: access_token and refresh_token are both long JWTs,
/// and dropping the id_token at sign-in only bought headroom, it didn't fix
/// the limit. Split anything longer across several credentials instead.
#[cfg_attr(not(windows), allow(dead_code))]
const MAX_UTF16_UNITS: usize = 1200;

/// Split at char boundaries so each piece fits `max_units` UTF-16 code units.
/// Returns one piece (possibly empty) for anything already short enough.
#[cfg_attr(not(windows), allow(dead_code))]
fn split_utf16(value: &str, max_units: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut chunk = String::new();
    let mut units = 0;
    for c in value.chars() {
        let w = c.len_utf16();
        if units + w > max_units {
            chunks.push(std::mem::take(&mut chunk));
            units = 0;
        }
        chunk.push(c);
        units += w;
    }
    chunks.push(chunk);
    chunks
}

#[cfg(windows)]
mod backend {
    use super::{split_utf16, MAX_UTF16_UNITS};

    const SERVICE: &str = "quota-widget";
    /// Written to the primary entry in place of the value when it was split.
    /// The control char can't collide with a real secret (JSON or API key).
    const MARKER: &str = "\u{1}quota-chunks:";

    fn part_name(provider: &str, i: usize) -> String {
        format!("{provider}__part{i}")
    }

    fn read_raw(name: &str) -> Option<String> {
        keyring::Entry::new(SERVICE, name).ok()?.get_password().ok()
    }

    fn write_raw(name: &str, value: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, name)
            .and_then(|e| e.set_password(value))
            .map_err(|e| e.to_string())
    }

    fn delete_raw(name: &str) -> Result<(), String> {
        match keyring::Entry::new(SERVICE, name).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// How many parts the stored value was split into, if it was split at all.
    fn chunk_count(raw: &str) -> Option<usize> {
        raw.strip_prefix(MARKER)?.trim().parse().ok()
    }

    pub fn get(_dir: &std::path::Path, provider: &str) -> Option<String> {
        let raw = read_raw(provider)?;
        let Some(n) = chunk_count(&raw) else {
            return Some(raw);
        };
        let mut out = String::new();
        for i in 0..n {
            // A missing part means a torn write; treat the secret as absent
            // rather than handing a truncated token to a provider.
            out.push_str(&read_raw(&part_name(provider, i))?);
        }
        Some(out)
    }

    /// Same contract as `get`, just `Result`-shaped for `secrets::backend`'s
    /// shared interface. Credential Manager access failures are not a
    /// currently-modeled "unavailable account" case the way an Android
    /// Keystore decrypt failure is (see the `target_os = "android"` arm
    /// below), so this still folds every failure into `Ok(None)` exactly as
    /// `get` always has — no behavior change on Windows.
    pub fn get_result(dir: &std::path::Path, provider: &str) -> Result<Option<String>, String> {
        Ok(get(dir, provider))
    }

    pub fn set(_dir: &std::path::Path, provider: &str, value: &str) -> Result<(), String> {
        let old_parts = read_raw(provider)
            .as_deref()
            .and_then(chunk_count)
            .unwrap_or(0);
        let chunks = split_utf16(value, MAX_UTF16_UNITS);
        let new_parts = if chunks.len() > 1 { chunks.len() } else { 0 };

        if new_parts == 0 {
            write_raw(provider, value)?;
        } else {
            // Parts first, marker last: a crash mid-write leaves the previous
            // value readable instead of a marker pointing at nothing.
            for (i, chunk) in chunks.iter().enumerate() {
                write_raw(&part_name(provider, i), chunk)?;
            }
            write_raw(provider, &format!("{MARKER}{new_parts}"))?;
        }
        for i in new_parts..old_parts {
            let _ = delete_raw(&part_name(provider, i));
        }
        Ok(())
    }

    pub fn clear(_dir: &std::path::Path, provider: &str) -> Result<(), String> {
        let parts = read_raw(provider)
            .as_deref()
            .and_then(chunk_count)
            .unwrap_or(0);
        delete_raw(provider)?;
        for i in 0..parts {
            delete_raw(&part_name(provider, i))?;
        }
        Ok(())
    }
}

/// Android Keystore-backed secret storage. An app-only, non-exportable AES
/// key generated in `AndroidKeyStore` (no `setUserAuthenticationRequired`,
/// matching the ADR's "usable by background work without biometric
/// confirmation") encrypts each entry; see
/// docs/adr/0006-…md.
///
/// `keyring` 3.x — pinned above for Windows — has no Android backend at all
/// (checked against its published Cargo.toml: no `target_os = "android"`
/// section exists before v4). v4 restructured the crate around
/// `keyring-core` "stores" and ships `android-native-keyring-store` as an
/// Android-only optional dependency, exposed through the crate's `v1`
/// feature as a facade documented to match the same `Entry::new(service,
/// user).get_password()/.set_password()/.delete_credential()` shape v3 used
/// — so this arm mirrors the Windows arm above call-for-call rather than
/// sharing code with it, despite depending on a different major version of
/// the same crate name (fine: Cargo resolves them independently since no
/// single compiled target pulls in both).
///
/// Unlike Windows, a failure here is a real, expected outcome — a corrupted
/// or hardware-rotated Keystore key leaves old ciphertext undecryptable — so
/// `get_result` surfaces it as `Err` rather than folding it into "absent".
/// `load_all_reporting_errors` below uses that distinction to mark the
/// affected account unavailable instead of silently "never configured".
#[cfg(target_os = "android")]
mod backend {
    const SERVICE: &str = "quota-widget";

    pub fn get_result(_dir: &std::path::Path, provider: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(SERVICE, provider).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn set(_dir: &std::path::Path, provider: &str, value: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, provider)
            .and_then(|e| e.set_password(value))
            .map_err(|e| e.to_string())
    }

    pub fn clear(_dir: &std::path::Path, provider: &str) -> Result<(), String> {
        match keyring::Entry::new(SERVICE, provider).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Everywhere else (Linux dev runs; would also cover iOS if this workspace
/// ever targeted it): the plaintext-file store, `Result`-wrapped so it can
/// sit behind the same `backend::get_result` interface as the platforms
/// above. It already collapses every read failure to `Ok(None)` internally
/// (`quota_core::secret_store::get`), so wrapping it here changes nothing —
/// only Android's arm ever returns a real `Err` from `get_result`.
#[cfg(not(any(windows, target_os = "android")))]
mod backend {
    pub use quota_core::secret_store::{clear, set};

    pub fn get_result(dir: &std::path::Path, provider: &str) -> Result<Option<String>, String> {
        Ok(quota_core::secret_store::get(dir, provider))
    }
}

pub fn get(dir: &Path, provider: &str) -> Option<String> {
    get_reporting_errors(dir, provider).unwrap_or(None)
}

/// Same lookup as `get`, but a genuine backend failure (currently only
/// possible on Android — see the `target_os = "android"` `backend` above) is
/// reported as `Err` instead of being folded into "no secret configured".
pub fn get_reporting_errors(dir: &Path, provider: &str) -> Result<Option<String>, String> {
    if !valid_key(provider) {
        return Ok(None);
    }
    backend::get_result(dir, provider)
}

pub fn set(dir: &Path, provider: &str, value: &str) -> Result<(), String> {
    if !valid_key(provider) {
        return Err("invalid secret key".into());
    }
    if value.trim().is_empty() {
        return clear(dir, provider);
    }
    backend::set(dir, provider, value.trim())
}

pub fn clear(dir: &Path, provider: &str) -> Result<(), String> {
    if !valid_key(provider) {
        return Err("invalid secret key".into());
    }
    backend::clear(dir, provider)
}

pub fn load_all(dir: &Path, config: &Config) -> HashMap<String, String> {
    quota_core::secret_store::secret_keys(config)
        .into_iter()
        .filter_map(|key| get(dir, &key).map(|v| (key, v)))
        .collect()
}

/// Every secret the configuration implies, split into what was found and
/// which keys the backend explicitly failed to read (as opposed to simply
/// having nothing stored). Used by Android's `mobile.rs` so an account whose
/// stored credential can no longer be decrypted is reported to the user as
/// unavailable rather than silently treated as never-configured — see the
/// `target_os = "android"` `backend` module's doc comment above. Desktop
/// keeps using the simpler `load_all`, since its backends never populate the
/// `failed` list (see `get_reporting_errors`).
pub fn load_all_reporting_errors(
    dir: &Path,
    config: &Config,
) -> (HashMap<String, String>, Vec<String>) {
    classify_secrets(quota_core::secret_store::secret_keys(config), |key| {
        get_reporting_errors(dir, key)
    })
}

/// The pure classification `load_all_reporting_errors` is built from,
/// factored out so it can be unit-tested against a fake backend — a real
/// Android Keystore decrypt failure can't be produced in this test suite.
fn classify_secrets(
    keys: Vec<String>,
    mut lookup: impl FnMut(&str) -> Result<Option<String>, String>,
) -> (HashMap<String, String>, Vec<String>) {
    let mut values = HashMap::new();
    let mut failed = Vec::new();
    for key in keys {
        match lookup(&key) {
            Ok(Some(value)) => {
                values.insert(key, value);
            }
            Ok(None) => {}
            Err(_) => failed.push(key),
        }
    }
    (values, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A decrypt failure (Android's real failure mode) must land the key in
    /// `failed`, not silently disappear as though nothing was ever pasted —
    /// this is the "marks affected accounts unavailable" acceptance
    /// criterion, tested against the pure classifier since no real Keystore
    /// is available here. Ciphertext is never touched by a read, only by
    /// `set`/`clear`, so "ciphertext preserved on a decrypt failure" holds by
    /// construction — there's no write path in this classification at all.
    #[test]
    fn a_backend_error_is_reported_as_failed_not_absent() {
        let keys = vec!["openrouter".to_string(), "elevenlabs".to_string()];
        let (values, failed) = classify_secrets(keys, |key| match key {
            "openrouter" => Ok(Some("sk-or-abc".to_string())),
            "elevenlabs" => Err("Keystore key unusable after factory reset".to_string()),
            other => panic!("unexpected key {other}"),
        });
        assert_eq!(
            values.get("openrouter").map(String::as_str),
            Some("sk-or-abc")
        );
        assert_eq!(failed, vec!["elevenlabs".to_string()]);
    }

    #[test]
    fn an_absent_secret_is_neither_a_value_nor_a_failure() {
        let (values, failed) = classify_secrets(vec!["openrouter".to_string()], |_| Ok(None));
        assert!(values.is_empty());
        assert!(failed.is_empty());
    }

    fn units(s: &str) -> usize {
        s.encode_utf16().count()
    }

    #[test]
    fn short_values_stay_in_one_piece() {
        let chunks = split_utf16("hello", 1200);
        assert_eq!(chunks, vec!["hello".to_string()]);
        assert_eq!(split_utf16("", 1200), vec![String::new()]);
    }

    #[test]
    fn long_values_split_under_the_limit_and_rejoin() {
        let value = "a".repeat(3000);
        let chunks = split_utf16(&value, 1200);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| units(c) <= 1200));
        assert_eq!(chunks.concat(), value);
    }

    #[test]
    fn splits_on_char_boundaries_for_surrogate_pairs() {
        // Each emoji is 2 UTF-16 units, so an odd limit must not split one.
        let value = "🙂".repeat(10);
        let chunks = split_utf16(&value, 3);
        assert!(chunks.iter().all(|c| units(c) <= 3));
        assert_eq!(chunks.concat(), value);
    }
}
