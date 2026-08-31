//! IPC for encrypted credential export/import (issue #151).
//!
//! `quota_core::transfer` builds/applies the plaintext bundle and
//! `quota_core::seal` encrypts/decrypts it under a passphrase — both pure and
//! already tested in-core. This file is the desktop adapter: it supplies the
//! bundle's ingredients (the shared configuration and the secret store),
//! reads and writes the sealed bytes at a path the frontend chose through a
//! native save/open dialog, and folds an import back into the running config
//! the same way `set_config` does.
//!
//! Desktop-only: the frontend's export/import controls and the
//! `tauri-plugin-dialog` dependency are both gated the same way.

use crate::secrets;
use crate::AppState;
use quota_core::config::Config;
use quota_core::seal;
use quota_core::transfer::{self, ApplyReport};
use std::sync::Arc;
use tauri::Emitter;

/// Seal every account — pasted keys included, OAuth/cookie accounts as
/// shells with no secret — under `passphrase` and write it to `path`.
#[tauri::command]
pub async fn export_credential_bundle(
    state: tauri::State<'_, Arc<AppState>>,
    path: String,
    passphrase: String,
) -> Result<(), String> {
    let cfg = state.config.read().await.clone();
    let (shared, _prefs) = cfg.split();
    let dir = state.config_dir.clone();
    let bundle = transfer::build_bundle(&shared, |key| secrets::get(&dir, key));
    let sealed = seal::seal(&bundle, &passphrase);
    std::fs::write(&path, sealed).map_err(|e| e.to_string())
}

/// Merge an opened `bundle` onto the running configuration and commit it —
/// the tail shared by the file import and the LAN pairing receiver. The
/// caller has already authenticated the bundle (passphrase or PAKE); this
/// folds it in exactly the way `set_config` does, so a transfer's result is
/// indistinguishable from an ordinary save. An unreadable config.json being
/// kept for recovery blocks this write like any other.
pub(crate) async fn apply_and_commit<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &Arc<AppState>,
    bundle: &transfer::CredentialBundle,
) -> Result<ApplyReport, String> {
    let cfg = state.config.read().await.clone();
    let (mut shared, prefs) = cfg.split();
    let dir = state.config_dir.clone();
    let report = transfer::apply_bundle(bundle, &mut shared, |key, value| {
        secrets::set(&dir, key, value)
    });

    let new_config = Config::from_parts(shared, prefs);
    new_config
        .save(&state.config_dir)
        .map_err(|e| e.to_string())?;
    *state.config.write().await = new_config.clone();
    let _ = app.emit("config", &new_config);
    state.refresh.notify_one();

    Ok(report)
}

/// Open the file at `path` under `passphrase` and merge its accounts onto the
/// current configuration, reporting what happened to each one.
///
/// A wrong passphrase or a tampered/truncated/unknown-version file is
/// refused by `seal::open` before anything here is touched, so a failed
/// import never alters existing accounts.
#[tauri::command]
pub async fn import_credential_bundle(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    path: String,
    passphrase: String,
) -> Result<ApplyReport, String> {
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let bundle = seal::open(&bytes, &passphrase).map_err(|e| e.to_string())?;

    apply_and_commit(&app, &state, &bundle).await
}
