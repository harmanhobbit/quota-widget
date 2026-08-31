// Frontend logic for LAN desktop pairing (issue #154), kept out of the
// component so the flow is unit-testable without a WebView — the same pattern
// as `qrTransfer.js`. The PAKE handshake, sealing and socket work all live in
// `quota_core::pairing` via the desktop shell's `lan_pairing_*` commands; this
// module only validates what the user typed and shapes the receiver's apply
// report into the same four-way summary the file import shows.

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

/** The pairing code format, matching what the receiver arms and shows. */
export const CODE_LENGTH = 6;

const isCode = (code) => /^\d{6}$/.test(code ?? '');

/**
 * Fold an apply report into the import summary's counts: how many accounts
 * were added, updated, awaiting sign-in, or could not be stored.
 */
export function summarizeReport(report) {
  const counts = { added: 0, updated: 0, needs_onboarding: 0, could_not_store: 0 };
  for (const outcome of Object.values(report?.accounts ?? {})) {
    counts[outcome.outcome] = (counts[outcome.outcome] ?? 0) + 1;
  }
  return counts;
}

/**
 * Sender side: send this device's accounts to the receiver at `address`
 * under `code`.
 *
 * @returns {Promise<{status: 'sent'} | {status: 'failed', msg: string}>}
 */
export async function runLanSend({ code, address, host }) {
  if (!isCode(code)) {
    return { status: 'failed', msg: 'Enter the 6-digit code the other device is showing.' };
  }
  if (!address?.trim()) {
    return { status: 'failed', msg: 'Enter the address the other device is showing.' };
  }
  try {
    await host.lanPairingSend(code, address.trim());
    return { status: 'sent' };
  } catch (e) {
    return { status: 'failed', msg: errText(e) };
  }
}

/**
 * Receiver side: arm `code` and wait for the sender. The command returns as
 * soon as the session is armed; the outcome arrives later on the
 * `lan-pairing` event, which the component routes through `handleLanResult`.
 */
export async function runLanReceiveWait({ code, host }) {
  if (!isCode(code)) {
    return { status: 'failed', msg: 'Generate or enter a 6-digit code first.' };
  }
  try {
    await host.lanPairingReceiveStart(code);
    return { status: 'waiting' };
  } catch (e) {
    return { status: 'failed', msg: errText(e) };
  }
}

/**
 * Handle the one `lan-pairing` event the armed session emits. A success
 * carries the apply report — the same shape the file import returns — which
 * becomes the four-way summary; the affected accounts are named so the
 * component can pull their fresh state in behind the open panel's back.
 */
export function handleLanResult(payload) {
  if (payload?.ok) {
    return { status: 'applied', summary: summarizeReport(payload.report), keys: Object.keys(payload.report?.accounts ?? {}) };
  }
  return { status: 'failed', msg: errText(payload?.error ?? 'The transfer did not complete.') };
}
