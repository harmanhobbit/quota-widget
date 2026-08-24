// The Android credential export/import flows (issue #152), kept out of the
// component so the whole lifecycle is unit-testable without a WebView — the
// same pattern as `credentialTest.js`. The cryptographic and merge rules live
// in quota_core (`transfer` / `seal`); what lives here is only the flow:
// validate the form, ask the user for a file, call the host command, and
// shape the outcome for display.
//
// Guarantees, regardless of how the host IPC behaves:
//   1. Neither function throws; both always resolve to a terminal
//      { status, … } result, so the UI is never stuck busy on any path.
//   2. A cancelled system dialog resolves to { status: 'cancelled' } and
//      nothing was asked of the host — the form stays exactly as it was.
//   3. Export validation (a passphrase was entered, and its confirmation
//      matches) happens before any dialog or command, so a typo'd passphrase
//      can never produce a backup the user cannot reopen.
//   4. The passphrase is handed to the host command and dropped from this
//      module — never stored, never logged.

/**
 * Group an `import_credentials` report's per-account outcomes into the lists
 * the import summary renders. `report.accounts` is the Rust `ApplyReport`'s
 * serialized IndexMap — a plain object here, preserving bundle order.
 */
export function summarizeImport(report) {
  const summary = { added: [], updated: [], needsSignIn: [], couldNotStore: [] };
  for (const [key, outcome] of Object.entries(report?.accounts ?? {})) {
    switch (outcome?.outcome) {
      case 'added':
        summary.added.push(key);
        break;
      case 'updated':
        summary.updated.push(key);
        break;
      // OAuth/cookie shells arrive signed out: the account is there, but the
      // user must complete provider onboarding before it reads anything.
      case 'needs_onboarding':
        summary.needsSignIn.push(key);
        break;
      case 'could_not_store':
        summary.couldNotStore.push({ key, reason: outcome.reason ?? 'unknown reason' });
        break;
    }
  }
  return summary;
}

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
 * Export every account to the encrypted file the user picks.
 *
 * @param {object}   opts
 * @param {string}   opts.passphrase  Passphrase to seal under.
 * @param {string}   opts.confirm     Retyped passphrase — must match.
 * @param {object}   opts.host        Injected: { pickExportDestination,
 *                                    exportCredentials }.
 * @returns {Promise<{status: 'done', msg: string}
 *                 | {status: 'cancelled'}
 *                 | {status: 'failed', msg: string}>}
 */
export async function runExport({ passphrase, confirm, host }) {
  if (!passphrase) {
    return { status: 'failed', msg: 'Choose a passphrase first — it is the only thing that can open the backup.' };
  }
  if (passphrase !== confirm) {
    return { status: 'failed', msg: 'The passphrases do not match.' };
  }
  let destination;
  try {
    destination = await host.pickExportDestination();
  } catch (e) {
    return { status: 'failed', msg: `The save dialog failed: ${errText(e)}` };
  }
  if (!destination) return { status: 'cancelled' };
  try {
    await host.exportCredentials(destination, passphrase);
  } catch (e) {
    return { status: 'failed', msg: errText(e) };
  }
  return {
    status: 'done',
    msg: 'Export written. Keep it safe — without the passphrase it cannot be opened, and the passphrase is not stored anywhere.',
  };
}

/**
 * Import the encrypted file the user picks, merging its accounts into the
 * current configuration.
 *
 * @param {object}   opts
 * @param {string}   opts.passphrase  Passphrase the file was sealed under.
 * @param {object}   opts.host        Injected: { pickImportSource,
 *                                    importCredentials }.
 * @returns {Promise<{status: 'done', summary: ReturnType<typeof summarizeImport>}
 *                 | {status: 'cancelled'}
 *                 | {status: 'failed', msg: string}>}
 */
export async function runImport({ passphrase, host }) {
  let source;
  try {
    source = await host.pickImportSource();
  } catch (e) {
    return { status: 'failed', msg: `The file picker failed: ${errText(e)}` };
  }
  if (!source) return { status: 'cancelled' };
  let report;
  try {
    report = await host.importCredentials(source, passphrase);
  } catch (e) {
    // The host refuses a wrong passphrase or a corrupt file before touching
    // anything — existing accounts are exactly as they were.
    return { status: 'failed', msg: errText(e) };
  }
  return { status: 'done', summary: summarizeImport(report) };
}
