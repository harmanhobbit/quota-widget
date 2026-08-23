//! Secret storage for pasted API keys / cookies.
//!
//! On Windows the Credential Manager is used (via the `keyring` crate), so the
//! portable EXE leaves no plaintext secrets on disk. On Android, an app-only
//! Android Keystore key is used instead via `keyring-core` plus
//! `android-native-keyring-store`, with the store registered as keyring-core's
//! default once at startup by `init_store` (the `keyring` crate's `v1`
//! facade can't be used on Android — see the `target_os = "android"` `backend`
//! module below). Elsewhere (Linux dev runs) secrets fall back to an
//! owner-only, atomically replaced JSON file in the config dir —
//! `quota_core::secret_store`, which is where the permission and atomicity
//! rules and their tests live.

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
/// confirmation") encrypts each entry; see docs/adr/0006-…md.
///
/// This arm talks to `keyring_core::Entry` directly rather than through the
/// `keyring` crate's `v1` compatibility facade. The facade's one-time
/// `set_credential_store` initializer (`keyring-4`'s `src/v1.rs`) has arms
/// only for macOS, Windows, and non-Android *nix — on `target_os =
/// "android"` it returns `Err(Invalid("platform", …))`, so
/// `keyring::Entry::new` short-circuits to `NoDefaultStore` before ever
/// touching the Keystore, and the facade simply cannot be used on Android.
/// `init_store` (below) registers `android-native-keyring-store`'s `Store`
/// as keyring-core's default once at startup instead, and this arm mirrors
/// the Windows arm above call-for-call against the same `Entry::new(service,
/// user).get_password()/.set_password()/.delete_credential()` shape — just
/// on `keyring_core::Entry` rather than `keyring::Entry`. The two crate
/// names never collide in one compiled target (Windows pins `keyring` 3,
/// Android pulls `keyring-core` + the store directly), so Cargo resolves
/// them independently.
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
        let entry = keyring_core::Entry::new(SERVICE, provider).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn set(_dir: &std::path::Path, provider: &str, value: &str) -> Result<(), String> {
        keyring_core::Entry::new(SERVICE, provider)
            .and_then(|e| e.set_password(value))
            .map_err(|e| e.to_string())
    }

    pub fn clear(_dir: &std::path::Path, provider: &str) -> Result<(), String> {
        match keyring_core::Entry::new(SERVICE, provider).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
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

/// Register the platform's credential store as keyring-core's default, once
/// at startup before any secret access. On Android this is
/// `android-native-keyring-store`'s Keystore-backed `Store`. On every other
/// target this is a no-op: Windows uses `keyring` 3, which manages its own
/// platform store, and the Linux/dev fallback uses
/// `quota_core::secret_store`'s plaintext file — neither routes through
/// keyring-core's default-store slot.
///
/// This call is load-bearing on Android because the `keyring` crate's `v1`
/// compatibility facade — the obvious thing to reach for, since its `Entry`
/// matches v3's shape — has a one-time `set_credential_store` initializer
/// with arms only for macOS, Windows, and non-Android *nix. On `target_os =
/// "android"` it returns `Err(Invalid("platform", …))`, so the facade's
/// `Entry::new` short-circuits to `NoDefaultStore` before ever touching the
/// Keystore — which is why secret storage on Android has never worked until
/// this call existed. Registering the store ourselves and using
/// `keyring_core::Entry` directly (see the `target_os = "android"` `backend`
/// above) is the fix. Called from `mobile.rs::run`'s setup closure before
/// the CI-seed block issues the first `secrets::set`.
///
/// The `ndk_context::initialize_android_context` call is equally
/// load-bearing, and its absence was the launch crash that followed the
/// registration fix. `Store::new()` reads the app context through
/// `ndk-context`, whose `android_context()` is
/// `ANDROID_CONTEXT.expect("android context was not initialized")` — a
/// *panic*, not an `Err`, when the context was never set. Nothing in the
/// tauri 2.11 / tao 0.35 / wry 0.55 stack sets it: their Android context
/// plumbing lives in tao's own `ndk_glue` module, and the only crates in
/// the tree that even reference `initialize_android_context` are
/// `ndk-context` itself and the store's Kotlin companion (which this app
/// does not wire up — it would mean hand-maintaining a Kotlin class against
/// Tauri's generated MainActivity). So the very first `Store::new()`
/// panicked inside the setup closure and took the process down before the
/// webview loaded. The store crate's README claims "Tauri Mobile" does this
/// initialization, but that is not true of the versions pinned here.
///
/// The fix is to copy tao's already-populated activity context across. tao
/// registers the activity (and holds its `GlobalRef`) in its `CONTEXTS` map
/// from the activity's creation until its destroy, and the setup closure
/// runs at the event loop's `Ready` event — strictly after creation — so
/// `main_android_context()` is populated by the time this runs. The store
/// only uses the context for `getSharedPreferences`, for which an Activity
/// context is correct. If no activity context exists yet we return `Err`
/// (logged at the call site) rather than risk the panic path: the app opens
/// without a credential store, which is the pre-registration behavior,
/// instead of not opening at all.
pub fn init_store() -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        // `initialize_android_context` asserts on re-entry
        // (`assert!(previous.is_none())` — "must be called exactly once"),
        // and a panic here is the very launch crash this function exists to
        // prevent, so make the call idempotent. The flag is set only after
        // the init succeeds, so an early `Err` return leaves a retry free
        // to attempt the init again.
        static NDK_CONTEXT_INIT: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if !NDK_CONTEXT_INIT.load(std::sync::atomic::Ordering::SeqCst) {
            let ctx = tauri::tao::platform::android::prelude::main_android_context()
                .ok_or_else(|| "no Android activity context available at startup".to_string())?;
            // Safety: tao's CONTEXTS map holds the activity's GlobalRef for
            // the activity's whole lifetime, so both raw pointers are valid
            // here and stay valid for as long as the store can use them.
            unsafe {
                ndk_context::initialize_android_context(ctx.java_vm, ctx.context_jobject);
            }
            NDK_CONTEXT_INIT.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let store = android_native_keyring_store::Store::new().map_err(|e| e.to_string())?;
        keyring_core::set_default_store(store);
    }
    Ok(())
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
#[cfg(any(test, mobile))]
pub fn load_all_reporting_errors(
    dir: &Path,
    config: &Config,
) -> (HashMap<String, String>, Vec<String>) {
    classify_secrets(quota_core::secret_store::secret_keys(config), |key| {
        get_reporting_errors(dir, key)
    })
}

/// The pure classification `load_all_reporting_errors` is built from. The
/// absent/present/unavailable contract itself lives in
/// `quota_core::secret_store::classify` (and is unit-tested there, by the cheap
/// Linux CI that never compiles this platform crate); this is the thin adapter
/// that runs the real per-platform backend through it.
#[cfg(any(test, mobile))]
fn classify_secrets(
    keys: Vec<String>,
    lookup: impl FnMut(&str) -> Result<Option<String>, String>,
) -> (HashMap<String, String>, Vec<String>) {
    quota_core::secret_store::classify(keys, lookup)
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
