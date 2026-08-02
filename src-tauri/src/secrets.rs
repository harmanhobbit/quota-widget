//! Secret storage for pasted API keys / cookies.
//!
//! On Windows the Credential Manager is used (via the `keyring` crate), so the
//! portable EXE leaves no plaintext secrets on disk. Elsewhere (Linux dev
//! runs) secrets fall back to a 0600 JSON file in the config dir.

use quota_core::config::Config;
use std::collections::HashMap;
use std::path::Path;

/// Keep keyring names predictable and reject accidental arbitrary entries.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'#' | b'_' | b'-'))
}

pub fn oauth_key(account: &str) -> String {
    format!("{account}_oauth")
}

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

#[cfg(not(windows))]
mod backend {
    use std::path::Path;

    fn path(dir: &Path) -> std::path::PathBuf {
        dir.join("secrets.json")
    }

    fn read(dir: &Path) -> serde_json::Map<String, serde_json::Value> {
        std::fs::read_to_string(path(dir))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn write(dir: &Path, map: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let p = path(dir);
        std::fs::write(&p, serde_json::to_string_pretty(map).unwrap())
            .map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn get(dir: &Path, provider: &str) -> Option<String> {
        read(dir).get(provider)?.as_str().map(String::from)
    }

    pub fn set(dir: &Path, provider: &str, value: &str) -> Result<(), String> {
        let mut map = read(dir);
        map.insert(provider.into(), value.into());
        write(dir, &map)
    }

    pub fn clear(dir: &Path, provider: &str) -> Result<(), String> {
        let mut map = read(dir);
        map.remove(provider);
        write(dir, &map)
    }
}

pub fn get(dir: &Path, provider: &str) -> Option<String> {
    if !valid_key(provider) {
        return None;
    }
    backend::get(dir, provider)
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
    config
        .providers
        .iter()
        .flat_map(|(key, p)| {
            let kind = p.kind.as_deref().unwrap_or(key);
            let mut keys = vec![key.clone()];
            if matches!(kind, "claude" | "codex") {
                keys.push(oauth_key(key));
            }
            keys
        })
        .filter_map(|key| get(dir, &key).map(|v| (key, v)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
