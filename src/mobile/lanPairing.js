// The Android LAN pairing flow (issue #155), kept out of the component so it
// is unit-testable without a WebView. The session rules and the shared
// validate/send/receive flow live in `src/lib/lanPairing.js` — the very same
// module desktop's Settings drives — and the host commands are the same
// `lan_pairing_*` set registered by both shells. What mobile adds is only
// how the receiver's outcome is presented: the phone shows the same
// four-way named-list summary its file/QR imports do
// (`summarizeImport`), so "needs sign-in" reads as provider onboarding and
// "could not store" is named per account with its reason.
import { handleLanResult } from '../lib/lanPairing.js';
import { summarizeImport } from './credentialTransfer.js';

/**
 * Handle the one `lan-pairing` event the armed session emits — the mobile
 * counterpart of desktop's `handleLanResult`, reusing it and re-shaping the
 * report for the phone's named-list summary.
 *
 * @returns {{status: 'applied', summary: ReturnType<typeof summarizeImport>, keys: string[]}
 *                 | {status: 'failed', msg: string}}
 */
export function handleLanPairingResult(payload) {
  const r = handleLanResult(payload);
  if (r.status === 'applied') {
    return { status: 'applied', summary: summarizeImport(payload.report), keys: r.keys };
  }
  return r;
}
