//! Secret storage for pasted API keys / cookies.
//!
//! On Windows the Credential Manager is used (via the `keyring` crate), so the
//! portable EXE leaves no plaintext secrets on disk. Elsewhere (Linux dev
//! runs) secrets fall back to a 0600 JSON file in the config dir.

use std::collections::HashMap;
use std::path::Path;

const PROVIDERS: &[&str] = &["claude", "codex", "openrouter", "hermes"];

#[cfg(windows)]
mod backend {
    const SERVICE: &str = "quota-widget";

    pub fn get(_dir: &std::path::Path, provider: &str) -> Option<String> {
        keyring::Entry::new(SERVICE, provider).ok()?.get_password().ok()
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
        std::fs::write(&p, serde_json::to_string_pretty(map).unwrap()).map_err(|e| e.to_string())?;
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
    backend::get(dir, provider)
}

pub fn set(dir: &Path, provider: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return clear(dir, provider);
    }
    backend::set(dir, provider, value.trim())
}

pub fn clear(dir: &Path, provider: &str) -> Result<(), String> {
    backend::clear(dir, provider)
}

pub fn load_all(dir: &Path) -> HashMap<String, String> {
    PROVIDERS
        .iter()
        .filter_map(|p| get(dir, p).map(|v| (p.to_string(), v)))
        .collect()
}
