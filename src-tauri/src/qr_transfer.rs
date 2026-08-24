//! IPC for the desktop→phone QR transfer (issue #156).
//!
//! `quota_core::transfer` builds the plaintext bundle, `quota_core::seal`
//! encrypts it under a passphrase, and `quota_core::qr_transfer` chunks the
//! sealed bytes into QR frames and renders each as SVG — all pure and
//! already tested in-core. This file is the desktop adapter: it supplies the
//! bundle's ingredients (the shared configuration and the secret store) and
//! a fresh random session id, and hands the frontend back ready-to-display
//! SVG markup to cycle through while the phone scans.
//!
//! Desktop-only, like `credential_transfer.rs` will be for the file export
//! (#151) — there is no matching import command here because this transport
//! only ever moves credentials off the desktop, never onto it.

use crate::secrets;
use crate::AppState;
use quota_core::{qr_transfer, seal, transfer};
use rand::RngCore;
use std::sync::Arc;

/// Seal every account under `passphrase` and render it as one or more QR
/// code frames (SVG markup) for the frontend to cycle through. Refuses with a
/// friendly message, rather than an unscannable oversized code sequence, when
/// the account set needs more frames than this transport supports.
#[tauri::command]
pub async fn qr_transfer_frames(
    state: tauri::State<'_, Arc<AppState>>,
    passphrase: String,
) -> Result<Vec<String>, String> {
    let cfg = state.config.read().await.clone();
    let (shared, _prefs) = cfg.split();
    let dir = state.config_dir.clone();
    let bundle = transfer::build_bundle(&shared, |key| secrets::get(&dir, key));
    let sealed = seal::seal(&bundle, &passphrase);

    let mut session = [0u8; 4];
    rand::thread_rng().fill_bytes(&mut session);

    qr_transfer::render_frames_svg(&sealed, session).map_err(|e| {
        format!(
            "{e} — pair over the local network or export a credentials file instead of \
             scanning this many accounts at once"
        )
    })
}
