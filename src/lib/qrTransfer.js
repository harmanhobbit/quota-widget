// Desktop side of the desktop→phone QR transfer (issue #156), kept out of the
// component so the whole lifecycle is unit-testable without a WebView — the
// same pattern as `credentialTransfer.js`. Chunking, encryption and QR
// rendering all live in `quota_core::qr_transfer` / `seal`, already tested
// there; this module only validates the passphrase and shapes the host's
// response for display. Nothing is picked here (unlike file export/import) —
// the frames render straight into the page for the phone to scan.

function errText(e) {
  if (e == null) return 'unknown error';
  if (typeof e === 'string') return e;
  if (e instanceof Error) return e.message;
  try {
    return JSON.stringify(e);
  } catch {
    return String(e);
  }
}

/**
 * Seal every account under the given passphrase and get back the QR frames
 * to display.
 *
 * @param {object}   opts
 * @param {string}   opts.passphrase  Passphrase to seal under.
 * @param {string}   opts.confirm     Retyped passphrase — must match.
 * @param {object}   opts.host        Injected: { qrTransferFrames }.
 * @returns {Promise<{status: 'ready', frames: string[]}
 *                 | {status: 'failed', msg: string}>}
 */
export async function runQrTransfer({ passphrase, confirm, host }) {
  if (!passphrase) {
    return { status: 'failed', msg: 'Choose a passphrase first — the phone will need it to open the code.' };
  }
  if (passphrase !== confirm) {
    return { status: 'failed', msg: 'The passphrases do not match.' };
  }
  let frames;
  try {
    frames = await host.qrTransferFrames(passphrase);
  } catch (e) {
    return { status: 'failed', msg: errText(e) };
  }
  return { status: 'ready', frames };
}
