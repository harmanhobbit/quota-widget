//! Sealed bundle format: encrypt a [`CredentialBundle`] to bytes under a
//! passphrase, and decrypt it back.
//!
//! This is the one wire format both the encrypted file export and the LAN
//! pairing path reuse — pairing just derives its key from a PAKE-negotiated
//! secret instead of a typed passphrase. Argon2id derives a symmetric key from
//! the passphrase; XChaCha20-Poly1305 is the AEAD. A magic value and version
//! number prefix the payload so a future format change is refused rather than
//! mis-parsed — the same posture as [`crate::shared_config::SharedConfig`]'s
//! `version` field.
//!
//! See `docs/adr/0008-credential-sync-is-direct-e2e-and-never-copies-oauth.md`.

use crate::transfer::CredentialBundle;
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    Key, XChaCha20Poly1305, XNonce,
};
use rand::RngCore;
use thiserror::Error;

const MAGIC_PASSPHRASE: [u8; 4] = *b"QWSB";
const MAGIC_KEY: [u8; 4] = *b"QWSK";
const FORMAT_VERSION: u16 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const KEY_LEN: usize = 32;
const HEADER_LEN: usize = MAGIC_PASSPHRASE.len() + 2 + SALT_LEN + NONCE_LEN;
const KEY_HEADER_LEN: usize = MAGIC_KEY.len() + 2 + NONCE_LEN;

// OWASP-recommended minimum for an interactive Argon2id KDF: 19 MiB, 2
// iterations, 1 degree of parallelism.
const ARGON2_MEMORY_KIB: u32 = 19_456;
const ARGON2_ITERATIONS: u32 = 2;
const ARGON2_PARALLELISM: u32 = 1;

/// Encrypt `bundle` to bytes under `passphrase`. The result embeds a random
/// salt and nonce, so sealing the same bundle twice with the same passphrase
/// produces different bytes.
pub fn seal(bundle: &CredentialBundle, passphrase: &str) -> Vec<u8> {
    let plaintext = serde_json::to_vec(bundle).expect("CredentialBundle always serializes");

    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    let key = derive_key(passphrase, &salt);

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from(nonce_bytes);

    let cipher = XChaCha20Poly1305::new(&Key::from(key));
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_ref())
        .expect("encryption under a freshly derived key and nonce cannot fail");

    let mut out = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    out.extend_from_slice(&MAGIC_PASSPHRASE);
    out.extend_from_slice(&FORMAT_VERSION.to_be_bytes());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    out
}

/// Decrypt bytes produced by [`seal`] back to a [`CredentialBundle`], under
/// `passphrase`.
///
/// A wrong passphrase and a tampered or truncated ciphertext both fail
/// AEAD authentication and are cryptographically indistinguishable from one
/// another by design — both surface as [`OpenError::Decrypt`]. A payload that
/// is too short to contain a header, that lacks the magic value, or that
/// carries a version this build does not understand is refused before any
/// decryption is attempted.
pub fn open(sealed: &[u8], passphrase: &str) -> Result<CredentialBundle, OpenError> {
    if sealed.len() < HEADER_LEN {
        return Err(OpenError::Truncated);
    }
    let (magic, rest) = sealed.split_at(4);
    if magic != MAGIC_PASSPHRASE {
        return Err(OpenError::BadMagic);
    }
    let (version_bytes, rest) = rest.split_at(2);
    let version = u16::from_be_bytes([version_bytes[0], version_bytes[1]]);
    if version != FORMAT_VERSION {
        return Err(OpenError::UnsupportedVersion(version));
    }
    let (salt, rest) = rest.split_at(SALT_LEN);
    let (nonce_bytes, ciphertext) = rest.split_at(NONCE_LEN);

    let key = derive_key(passphrase, salt);
    let cipher = XChaCha20Poly1305::new(&Key::from(key));
    let nonce_array: [u8; NONCE_LEN] = nonce_bytes
        .try_into()
        .expect("nonce_bytes was split to exactly NONCE_LEN bytes");
    let nonce = XNonce::from(nonce_array);
    let plaintext = cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|_| OpenError::Decrypt)?;

    serde_json::from_slice(&plaintext).map_err(OpenError::Malformed)
}

fn derive_key(passphrase: &str, salt: &[u8]) -> [u8; KEY_LEN] {
    let params = Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        Some(KEY_LEN),
    )
    .expect("static Argon2id parameters are valid");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; KEY_LEN];
    argon2
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .expect("Argon2id derivation with valid static parameters cannot fail");
    key
}

/// Encrypt `bundle` to bytes under a raw 32-byte key produced by the PAKE
/// handshake. No Argon2id KDF is applied — the key is used directly with
/// XChaCha20-Poly1305. The result embeds a random nonce; a different magic
/// value distinguishes it from a passphrase-sealed bundle.
pub fn seal_with_key(bundle: &CredentialBundle, key: &[u8; KEY_LEN]) -> Vec<u8> {
    let plaintext = serde_json::to_vec(bundle).expect("CredentialBundle always serializes");

    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = XNonce::from(nonce_bytes);

    let cipher = XChaCha20Poly1305::new(&Key::from(*key));
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_ref())
        .expect("encryption under a freshly derived key and nonce cannot fail");

    let mut out = Vec::with_capacity(KEY_HEADER_LEN + ciphertext.len());
    out.extend_from_slice(&MAGIC_KEY);
    out.extend_from_slice(&FORMAT_VERSION.to_be_bytes());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    out
}

/// Decrypt bytes produced by [`seal_with_key`] back to a [`CredentialBundle`],
/// under the same raw 32-byte key.
pub fn open_with_key(sealed: &[u8], key: &[u8; KEY_LEN]) -> Result<CredentialBundle, OpenError> {
    if sealed.len() < KEY_HEADER_LEN {
        return Err(OpenError::Truncated);
    }
    let (magic, rest) = sealed.split_at(4);
    if magic != MAGIC_KEY {
        return Err(OpenError::BadMagic);
    }
    let (version_bytes, rest) = rest.split_at(2);
    let version = u16::from_be_bytes([version_bytes[0], version_bytes[1]]);
    if version != FORMAT_VERSION {
        return Err(OpenError::UnsupportedVersion(version));
    }
    let (nonce_bytes, ciphertext) = rest.split_at(NONCE_LEN);

    let cipher = XChaCha20Poly1305::new(&Key::from(*key));
    let nonce_array: [u8; NONCE_LEN] = nonce_bytes
        .try_into()
        .expect("nonce_bytes was split to exactly NONCE_LEN bytes");
    let nonce = XNonce::from(nonce_array);
    let plaintext = cipher
        .decrypt(&nonce, ciphertext)
        .map_err(|_| OpenError::Decrypt)?;

    serde_json::from_slice(&plaintext).map_err(OpenError::Malformed)
}

#[derive(Debug, Error)]
pub enum OpenError {
    #[error("sealed bundle is truncated")]
    Truncated,
    #[error("not a sealed credential bundle")]
    BadMagic,
    #[error("unsupported sealed bundle version: {0}")]
    UnsupportedVersion(u16),
    #[error("wrong passphrase, or the sealed bundle was tampered with or truncated")]
    Decrypt,
    #[error("decrypted payload is not a valid credential bundle: {0}")]
    Malformed(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProviderConfig;
    use crate::transfer::BundleAccount;

    fn sample_bundle() -> CredentialBundle {
        let mut bundle = CredentialBundle::default();
        bundle.accounts.insert(
            "openrouter".into(),
            BundleAccount {
                config: ProviderConfig {
                    enabled: true,
                    kind: Some("openrouter".into()),
                    label: Some("Personal".into()),
                    ..ProviderConfig::default()
                },
                secret: Some("sk-or-v1-abc".into()),
            },
        );
        bundle
    }

    #[test]
    fn round_trips_under_the_same_passphrase() {
        let bundle = sample_bundle();
        let sealed = seal(&bundle, "correct horse battery staple");
        let opened = open(&sealed, "correct horse battery staple").unwrap();
        assert_eq!(opened, bundle);
    }

    #[test]
    fn sealing_twice_produces_different_bytes() {
        let bundle = sample_bundle();
        let a = seal(&bundle, "same passphrase");
        let b = seal(&bundle, "same passphrase");
        assert_ne!(a, b);
    }

    #[test]
    fn wrong_passphrase_is_refused_cleanly() {
        let bundle = sample_bundle();
        let sealed = seal(&bundle, "right passphrase");
        let err = open(&sealed, "wrong passphrase").unwrap_err();
        assert!(matches!(err, OpenError::Decrypt));
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let bundle = sample_bundle();
        let mut sealed = seal(&bundle, "a passphrase");
        let last = sealed.len() - 1;
        sealed[last] ^= 0xFF;
        let err = open(&sealed, "a passphrase").unwrap_err();
        assert!(matches!(err, OpenError::Decrypt));
    }

    #[test]
    fn truncated_ciphertext_is_rejected_without_panicking() {
        let bundle = sample_bundle();
        let sealed = seal(&bundle, "a passphrase");
        for len in [0, 1, HEADER_LEN - 1, HEADER_LEN, HEADER_LEN + 5] {
            let truncated = &sealed[..len.min(sealed.len())];
            let result = open(truncated, "a passphrase");
            assert!(result.is_err());
        }
    }

    #[test]
    fn unknown_version_is_refused_not_misparsed() {
        let bundle = sample_bundle();
        let mut sealed = seal(&bundle, "a passphrase");
        sealed[4..6].copy_from_slice(&99u16.to_be_bytes());
        let err = open(&sealed, "a passphrase").unwrap_err();
        assert!(matches!(err, OpenError::UnsupportedVersion(99)));
    }

    #[test]
    fn bad_magic_is_refused() {
        let bundle = sample_bundle();
        let mut sealed = seal(&bundle, "a passphrase");
        sealed[0..4].copy_from_slice(b"NOPE");
        let err = open(&sealed, "a passphrase").unwrap_err();
        assert!(matches!(err, OpenError::BadMagic));
    }

    #[test]
    fn key_seal_round_trips() {
        let bundle = sample_bundle();
        let key = [0xABu8; KEY_LEN];
        let sealed = seal_with_key(&bundle, &key);
        let opened = open_with_key(&sealed, &key).unwrap();
        assert_eq!(opened, bundle);
    }

    #[test]
    fn key_seal_wrong_key_is_refused() {
        let bundle = sample_bundle();
        let key = [0xABu8; KEY_LEN];
        let wrong_key = [0xCDu8; KEY_LEN];
        let sealed = seal_with_key(&bundle, &key);
        let err = open_with_key(&sealed, &wrong_key).unwrap_err();
        assert!(matches!(err, OpenError::Decrypt));
    }

    #[test]
    fn key_seal_tampered_ciphertext_is_rejected() {
        let bundle = sample_bundle();
        let key = [0xABu8; KEY_LEN];
        let mut sealed = seal_with_key(&bundle, &key);
        let last = sealed.len() - 1;
        sealed[last] ^= 0xFF;
        let err = open_with_key(&sealed, &key).unwrap_err();
        assert!(matches!(err, OpenError::Decrypt));
    }

    #[test]
    fn key_seal_is_distinct_from_passphrase_seal() {
        let bundle = sample_bundle();
        let passphrase_sealed = seal(&bundle, "a passphrase");
        let key = [0xABu8; KEY_LEN];
        let key_sealed = seal_with_key(&bundle, &key);
        assert_ne!(passphrase_sealed[0..4], key_sealed[0..4]);
        let err = open(&key_sealed, "a passphrase").unwrap_err();
        assert!(matches!(err, OpenError::BadMagic));
        let err = open_with_key(&passphrase_sealed, &key).unwrap_err();
        assert!(matches!(err, OpenError::BadMagic));
    }
}
