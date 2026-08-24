// Android side of the desktop→phone QR transfer (issue #156), kept out of
// the component so the whole lifecycle is unit-testable without a WebView —
// the same pattern as `credentialTransfer.js`. Frame chunking/reassembly
// lives in `quota_core::qr_transfer`, already tested there; the host commands
// (`qrScanReset`/`qrScanFrame`/`qrScanFinish`) fold each scanned frame into
// that collector and, once complete, open and apply the reassembled bundle
// the same way a picked file import does — `summarizeImport` from
// `credentialTransfer.js` renders the identical `ApplyReport` shape either
// way.
//
// Guarantees, regardless of how the host or camera behave:
//   1. `runQrScan` and `finishQrImport` never throw; both always resolve to a
//      terminal { status, … } result.
//   2. Denied camera permission or a cancelled scan resolves to
//      { status: 'cancelled' } without ever calling `qrScanFrame`.
//   3. The passphrase is handed to the host command and dropped from this
//      module — never stored, never logged.

import { summarizeImport } from './credentialTransfer.js';

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
 * Scan QR frames until the transfer is fully collected (or the user cancels,
 * or permission is refused). Calls `onProgress(status)` — `{ have, total,
 * complete }`, the `quota_core::qr_transfer::FrameStatus` shape — after every
 * frame the host recognises, so the caller can render "captured N of M".
 *
 * @param {object}   opts
 * @param {object}   opts.host        Injected: { qrCheckPermissions,
 *                                    qrRequestPermissions, qrScanReset,
 *                                    qrScan, qrScanFrame, qrCancelScan }.
 * @param {(status: {have: number, total: number, complete: boolean}) => void} [opts.onProgress]
 * @returns {Promise<{status: 'complete'}
 *                 | {status: 'cancelled'}
 *                 | {status: 'failed', msg: string}>}
 */
export async function runQrScan({ host, onProgress }) {
  let permission;
  try {
    permission = await host.qrCheckPermissions();
    if (permission !== 'granted') {
      permission = await host.qrRequestPermissions();
    }
  } catch (e) {
    return { status: 'failed', msg: `Could not check camera permission: ${errText(e)}` };
  }
  if (permission !== 'granted') {
    return { status: 'cancelled' };
  }

  try {
    await host.qrScanReset();
  } catch (e) {
    return { status: 'failed', msg: errText(e) };
  }

  for (;;) {
    let scanned;
    try {
      // 'QR_CODE' is the barcode-scanner plugin's `Format.QRCode` enum value —
      // passed as the literal string rather than importing the plugin's enum,
      // so this module (and `host.js`) never statically import the plugin
      // itself; only the lazy `qrScan` wrapper does.
      scanned = await host.qrScan({ windowed: true, formats: ['QR_CODE'] });
    } catch {
      // A cancelled scan (user backed out of the camera view) and a genuine
      // scanner failure are indistinguishable from the plugin's rejection
      // alone — either way, stop and leave the caller free to retry.
      return { status: 'cancelled' };
    }
    let status;
    try {
      status = await host.qrScanFrame(scanned.content);
    } catch (e) {
      return { status: 'failed', msg: errText(e) };
    }
    onProgress?.(status);
    if (status.complete) {
      return { status: 'complete' };
    }
  }
}

/**
 * Open the fully-scanned bundle under `passphrase` and merge its accounts
 * into the current configuration.
 *
 * @param {object}   opts
 * @param {string}   opts.passphrase  Passphrase the QR transfer was sealed under.
 * @param {object}   opts.host        Injected: { qrScanFinish }.
 * @returns {Promise<{status: 'done', summary: ReturnType<typeof summarizeImport>}
 *                 | {status: 'failed', msg: string}>}
 */
export async function finishQrImport({ passphrase, host }) {
  let report;
  try {
    report = await host.qrScanFinish(passphrase);
  } catch (e) {
    // The host refuses a wrong passphrase or a corrupt/tampered payload
    // before touching anything — existing accounts are exactly as they were.
    return { status: 'failed', msg: errText(e) };
  }
  return { status: 'done', summary: summarizeImport(report) };
}
