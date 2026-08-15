//! Naming rules for secret entries, and the plaintext-file store used where no
//! OS keychain is wired up (Linux dev runs; Windows uses the Credential
//! Manager instead, in `src-tauri/src/secrets.rs`).
//!
//! The file holds API keys, session cookies and OAuth tokens in the clear, so
//! the POSIX mode is the only thing keeping other local users out. That has
//! two consequences the implementation is built around:
//!
//! * the mode has to be right *at creation*, not fixed up afterwards — a
//!   `write` + `chmod` pair leaves the secret world-readable for the window in
//!   between;
//! * a filesystem that ignores modes (exFAT, some network mounts) has to be
//!   reported rather than silently accepted, because nothing in the UI would
//!   otherwise reveal that the store is not actually protected.
//!
//! Updates go to a temp file in the same directory and are renamed over the
//! store, so a crash or a full disk leaves either the whole previous store or
//! the whole new one — never half a JSON document.

use crate::config::Config;
use std::path::{Path, PathBuf};

/// Keep entry names predictable and reject accidental arbitrary entries.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'#' | b'_' | b'-'))
}

/// The OAuth entry belonging to one configured account. Accounts are keyed by
/// their config map key, so two Claude accounts get two distinct secrets.
pub fn oauth_key(account: &str) -> String {
    format!("{account}_oauth")
}

/// Every entry name the configuration implies, in config order: one per
/// account, plus an OAuth entry for the kinds that sign in rather than paste a
/// key.
pub fn secret_keys(config: &Config) -> Vec<String> {
    config
        .providers
        .iter()
        .flat_map(|(key, p)| {
            let kind = p.kind.as_deref().unwrap_or(key);
            let mut keys = vec![key.clone()];
            if matches!(kind, "claude" | "codex" | "grok") {
                keys.push(oauth_key(key));
            }
            keys
        })
        .collect()
}

/// The plaintext JSON store inside the config dir.
pub fn store_path(dir: &Path) -> PathBuf {
    dir.join("secrets.json")
}

/// Owner-only: nothing readable, writable or executable by group or other.
#[cfg(unix)]
pub fn is_owner_only(mode: u32) -> bool {
    mode & 0o077 == 0
}

fn temp_path(dir: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(".secrets.json.{}.{n}.tmp", std::process::id()))
}

/// Create `path`, failing if it already exists, with owner-only permissions
/// applied by the `open` call itself so no byte of the secret is ever written
/// to a file other users could read.
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

/// Confirm the freshly created file really is owner-only. `create_private`
/// asks for 0600, but a filesystem without POSIX permissions just ignores the
/// request, and the caller must not be told the store is protected when it is
/// not.
#[cfg(unix)]
fn verify_private(file: &std::fs::File, path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = file
        .metadata()
        .map_err(|e| format!("could not read permissions of {} ({e})", path.display()))?
        .permissions()
        .mode()
        & 0o777;
    if is_owner_only(mode) {
        return Ok(());
    }
    Err(format!(
        "refusing to write plaintext secrets to {}: the filesystem left the file \
         mode {mode:04o} instead of 0600, so other users on this machine could \
         read your API keys and OAuth tokens",
        path.display()
    ))
}

#[cfg(not(unix))]
fn verify_private(_file: &std::fs::File, _path: &Path) -> Result<(), String> {
    Ok(())
}

/// Create the config dir, owner-only where the platform has modes.
fn create_dir(dir: &Path) -> Result<(), String> {
    if dir.is_dir() {
        return Ok(());
    }
    let mut b = std::fs::DirBuilder::new();
    b.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    b.create(dir)
        .map_err(|e| format!("could not create {} ({e})", dir.display()))
}

type Map = serde_json::Map<String, serde_json::Value>;

/// Read the store. `Ok(None)` means there is nothing there yet; an unreadable
/// or unparseable store is an error so that `set`/`clear` refuse to overwrite
/// secrets they could not read.
fn read_map(dir: &Path) -> Result<Option<Map>, String> {
    let p = store_path(dir);
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("could not read {} ({e})", p.display())),
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("{} is not valid JSON ({e})", p.display()))
}

/// Replace the store with `map`, atomically and owner-only.
fn write_map(dir: &Path, map: &Map) -> Result<(), String> {
    use std::io::Write;

    create_dir(dir)?;
    let final_path = store_path(dir);
    let tmp = temp_path(dir);
    // A temp file from a killed earlier run would otherwise block create_new.
    let _ = std::fs::remove_file(&tmp);

    let mut file =
        create_private(&tmp).map_err(|e| format!("could not create {} ({e})", tmp.display()))?;

    // Permissions first, secret bytes second: if the mode did not take, the
    // file we discard here is still empty.
    if let Err(e) = verify_private(&file, &tmp) {
        drop(file);
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }

    let body = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
    let written = file
        .write_all(body.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        // Durability matters more than speed here: without the flush a crash
        // can rename an empty file over a store full of tokens.
        .and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("could not write {} ({e})", tmp.display()));
    }

    if let Err(e) = std::fs::rename(&tmp, &final_path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("could not replace {} ({e})", final_path.display()));
    }
    // Best effort: makes the rename itself durable. Not every platform allows
    // opening a directory, and the rename has already happened either way.
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

pub fn get(dir: &Path, key: &str) -> Option<String> {
    read_map(dir)
        .ok()
        .flatten()?
        .get(key)?
        .as_str()
        .map(String::from)
}

pub fn set(dir: &Path, key: &str, value: &str) -> Result<(), String> {
    let mut map = read_map(dir)?.unwrap_or_default();
    map.insert(key.into(), value.into());
    write_map(dir, &map)
}

pub fn clear(dir: &Path, key: &str) -> Result<(), String> {
    let Some(mut map) = read_map(dir)? else {
        return Ok(());
    };
    if map.remove(key).is_none() {
        return Ok(());
    }
    write_map(dir, &map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn chmod(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// root ignores the mode bits, so the "make it unwritable" failure tests
    /// cannot be staged there. Probed rather than asked, to avoid a libc dep.
    #[cfg(unix)]
    fn writes_ignore_permissions(dir: &Path) -> bool {
        let probe = dir.join("probe");
        chmod(dir, 0o500);
        let ignored = std::fs::write(&probe, b"x").is_ok();
        chmod(dir, 0o700);
        let _ = std::fs::remove_file(&probe);
        ignored
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n != "secrets.json")
            .collect();
        names.sort();
        names
    }

    #[test]
    fn stores_and_reads_back_a_secret() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), "openrouter", "sk-or-v1-abc").unwrap();
        assert_eq!(
            get(dir.path(), "openrouter"),
            Some("sk-or-v1-abc".to_string())
        );
        assert_eq!(get(dir.path(), "elevenlabs"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_new_store_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), "openrouter", "sk-or-v1-abc").unwrap();
        assert_eq!(mode_of(&store_path(dir.path())), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn the_config_dir_itself_is_created_owner_only() {
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("quota-widget");
        set(&dir, "openrouter", "sk-or-v1-abc").unwrap();
        assert_eq!(mode_of(&dir), 0o700);
    }

    /// The mode has to be in place before any secret byte lands, so check the
    /// file the writer creates while it is still empty.
    #[cfg(unix)]
    #[test]
    fn the_store_file_is_owner_only_before_any_bytes_are_written() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("fresh");
        let file = create_private(&p).unwrap();
        assert_eq!(mode_of(&p), 0o600);
        assert_eq!(file.metadata().unwrap().len(), 0);
        // Second attempt must not silently reuse someone else's file.
        assert!(create_private(&p).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_loosely_permissioned_store_is_tightened_by_the_next_write() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), "openrouter", "one").unwrap();
        chmod(&store_path(dir.path()), 0o644);
        set(dir.path(), "elevenlabs", "two").unwrap();
        assert_eq!(mode_of(&store_path(dir.path())), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn group_and_world_bits_are_rejected() {
        assert!(is_owner_only(0o600));
        assert!(is_owner_only(0o400));
        for mode in [0o644, 0o604, 0o660, 0o606, 0o777, 0o601] {
            assert!(!is_owner_only(mode), "{mode:04o} should be rejected");
        }
    }

    #[test]
    fn updates_keep_earlier_secrets_and_leave_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), "openrouter", "one").unwrap();
        set(dir.path(), "elevenlabs", "two").unwrap();
        set(dir.path(), "openrouter", "one-rotated").unwrap();
        assert_eq!(get(dir.path(), "openrouter").unwrap(), "one-rotated");
        assert_eq!(get(dir.path(), "elevenlabs").unwrap(), "two");
        assert_eq!(leftovers(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn clearing_removes_one_secret_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), "openrouter", "one").unwrap();
        set(dir.path(), "elevenlabs", "two").unwrap();
        clear(dir.path(), "openrouter").unwrap();
        assert_eq!(get(dir.path(), "openrouter"), None);
        assert_eq!(get(dir.path(), "elevenlabs").unwrap(), "two");
        // Still a complete document, not a truncated one.
        let text = std::fs::read_to_string(store_path(dir.path())).unwrap();
        let map: Map = serde_json::from_str(&text).unwrap();
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn clearing_an_absent_secret_does_not_create_a_store() {
        let dir = tempfile::tempdir().unwrap();
        clear(dir.path(), "openrouter").unwrap();
        assert!(!store_path(dir.path()).exists());
    }

    /// A write that cannot complete must leave the previous store whole rather
    /// than a partial JSON document.
    #[cfg(unix)]
    #[test]
    fn a_failed_write_preserves_the_previous_store_intact() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), "openrouter", "one").unwrap();
        let before = std::fs::read_to_string(store_path(dir.path())).unwrap();
        if writes_ignore_permissions(dir.path()) {
            return; // running as root
        }

        chmod(dir.path(), 0o500);
        let err = set(dir.path(), "elevenlabs", "two").unwrap_err();
        let after = std::fs::read_to_string(store_path(dir.path())).unwrap();
        chmod(dir.path(), 0o700);

        assert!(err.contains("could not create"), "{err}");
        assert_eq!(after, before);
        let map: Map = serde_json::from_str(&after).unwrap();
        assert_eq!(map["openrouter"], "one");
        assert!(!map.contains_key("elevenlabs"));
        assert_eq!(leftovers(dir.path()), Vec::<String>::new());
    }

    #[test]
    fn an_unwritable_location_is_reported_and_writes_nothing() {
        let parent = tempfile::tempdir().unwrap();
        let blocker = parent.path().join("not-a-dir");
        std::fs::write(&blocker, b"i am a file").unwrap();
        let dir = blocker.join("quota-widget");

        let err = set(&dir, "openrouter", "sk-or-v1-abc").unwrap_err();
        assert!(err.contains("not-a-dir"), "{err}");
        assert!(!store_path(&dir).exists());
        assert_eq!(std::fs::read_to_string(&blocker).unwrap(), "i am a file");
    }

    /// Overwriting a store we could not parse would silently drop every other
    /// secret in it, so refuse instead.
    #[test]
    fn a_corrupt_store_is_reported_rather_than_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let p = store_path(dir.path());
        std::fs::write(&p, b"{\"openrouter\": \"one\"").unwrap();

        let err = set(dir.path(), "elevenlabs", "two").unwrap_err();
        assert!(err.contains("not valid JSON"), "{err}");
        assert!(clear(dir.path(), "openrouter").is_err());
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "{\"openrouter\": \"one\""
        );
        assert_eq!(get(dir.path(), "openrouter"), None);
    }

    #[test]
    fn an_empty_store_file_is_treated_as_no_secrets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(store_path(dir.path()), b"").unwrap();
        assert_eq!(get(dir.path(), "openrouter"), None);
        set(dir.path(), "openrouter", "one").unwrap();
        assert_eq!(get(dir.path(), "openrouter").unwrap(), "one");
    }

    #[test]
    fn secret_keys_cover_every_account_and_its_oauth_entry() {
        let mut config = Config::default();
        config.providers.insert(
            "claude_work".into(),
            ProviderConfig {
                kind: Some("claude".into()),
                ..Default::default()
            },
        );
        let keys = secret_keys(&config);

        assert!(keys.contains(&"claude".to_string()));
        assert!(keys.contains(&"claude_oauth".to_string()));
        assert!(keys.contains(&"codex_oauth".to_string()));
        assert!(keys.contains(&"grok_oauth".to_string()));
        assert!(keys.contains(&"claude_work".to_string()));
        assert!(keys.contains(&"claude_work_oauth".to_string()));
        // Pasted-key providers get no OAuth entry.
        assert!(keys.contains(&"openrouter".to_string()));
        assert!(!keys.contains(&"openrouter_oauth".to_string()));
        assert!(keys.iter().all(|k| valid_key(k)), "{keys:?}");
    }

    #[test]
    fn per_account_secrets_stay_separate_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        set(dir.path(), &oauth_key("claude"), "token-personal").unwrap();
        set(dir.path(), &oauth_key("claude_work"), "token-work").unwrap();
        assert_eq!(get(dir.path(), "claude_oauth").unwrap(), "token-personal");
        assert_eq!(get(dir.path(), "claude_work_oauth").unwrap(), "token-work");
    }

    #[test]
    fn keys_are_validated() {
        assert!(valid_key("claude_oauth"));
        assert!(valid_key("anthropic_admin#2"));
        assert!(!valid_key(""));
        assert!(!valid_key("../escape"));
        assert!(!valid_key("has space"));
        assert!(!valid_key(&"a".repeat(129)));
    }
}
