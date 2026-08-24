<script>
  // Credential export & import (issue #152): an encrypted file that moves
  // accounts between this device and a desktop (or another phone), and
  // doubles as a backup. The seal/open and merge rules live in quota_core —
  // this component only collects the passphrase, asks the user for a file via
  // the system dialog, and renders the report. OAuth and cookie accounts
  // travel as shells (ADR-0008): they arrive signed out and land in provider
  // onboarding above, which is what the "needs sign-in" summary line means.
  import { runExport, runImport } from './credentialTransfer.js';
  import { runQrScan, finishQrImport } from './qrImport.js';
  import {
    pickExportDestination,
    pickImportSource,
    exportCredentials,
    importCredentials,
    qrCheckPermissions,
    qrRequestPermissions,
    qrScanReset,
    qrScan,
    qrCancelScan,
    qrScanFrame,
    qrScanFinish,
  } from '../lib/host.js';

  let {
    // Called after an import actually landed accounts, so the parent can
    // re-read the configuration and refresh the usage list.
    onImported,
  } = $props();

  let mode = $state(null); // null | 'export' | 'import' | 'qr-scanning' | 'qr-passphrase'
  let passphrase = $state('');
  let confirm = $state('');
  let busy = $state(false);
  let error = $state('');
  let doneMsg = $state('');
  let summary = $state(null);
  // Desktop→phone QR transfer (issue #156): progress while `qr-scanning`,
  // populated by `runQrScan`'s onProgress callback for a "captured N of M"
  // caption — the same `quota_core::qr_transfer::FrameStatus` shape the host
  // command returns.
  let qrProgress = $state(null);

  function open(which) {
    mode = which;
    passphrase = '';
    confirm = '';
    error = '';
    doneMsg = '';
    summary = null;
  }

  async function doExport() {
    busy = true;
    error = '';
    doneMsg = '';
    const r = await runExport({
      passphrase,
      confirm,
      host: { pickExportDestination, exportCredentials },
    });
    busy = false;
    if (r.status === 'failed') {
      error = r.msg;
    } else if (r.status === 'done') {
      // Back to the action row with the confirmation left on screen; the
      // passphrase is never kept around once it has done its job.
      mode = null;
      doneMsg = r.msg;
      passphrase = '';
      confirm = '';
    }
    // 'cancelled' leaves the form exactly as the user filled it.
  }

  async function doImport() {
    busy = true;
    error = '';
    summary = null;
    const r = await runImport({
      passphrase,
      host: { pickImportSource, importCredentials },
    });
    busy = false;
    if (r.status === 'failed') {
      error = r.msg;
    } else if (r.status === 'done') {
      mode = null;
      summary = r.summary;
      passphrase = '';
      // Only accounts that landed are worth a reload: an import whose every
      // account failed to store changed nothing on this device.
      const landed =
        summary.added.length + summary.updated.length + summary.needsSignIn.length > 0;
      if (landed) await onImported?.();
    }
  }

  async function startQrScan() {
    mode = 'qr-scanning';
    error = '';
    qrProgress = null;
    const r = await runQrScan({
      host: { qrCheckPermissions, qrRequestPermissions, qrScanReset, qrScan, qrScanFrame },
      onProgress: (status) => {
        qrProgress = status;
      },
    });
    if (r.status === 'failed') {
      mode = null;
      error = r.msg;
    } else if (r.status === 'cancelled') {
      mode = null;
    } else {
      // Complete: ask for the passphrase before opening what was scanned.
      mode = 'qr-passphrase';
      passphrase = '';
    }
  }

  function cancelQrScan() {
    // Aborts the in-flight `qrScan()` promise `runQrScan`'s loop is awaiting;
    // its rejection is what turns that loop's result into 'cancelled'.
    qrCancelScan();
  }

  async function doQrFinish() {
    busy = true;
    error = '';
    summary = null;
    const r = await finishQrImport({ passphrase, host: { qrScanFinish } });
    busy = false;
    if (r.status === 'failed') {
      error = r.msg;
    } else {
      mode = null;
      summary = r.summary;
      passphrase = '';
      const landed =
        summary.added.length + summary.updated.length + summary.needsSignIn.length > 0;
      if (landed) await onImported?.();
    }
  }
</script>

<section class="credential-transfer">
  <h2>Credential export</h2>
  <p class="note">
    Back up your accounts to an encrypted file, or restore them from one — for
    example one exported on your desktop. The file only opens with the
    passphrase you seal it with; lose it and the backup is unrecoverable.
    Claude, Codex and Hermes accounts travel without their sign-in and ask you
    to sign in again here, so a session is never shared between devices.
  </p>
  {#if mode === null}
    <div class="row transfer-actions">
      <button onclick={() => open('export')}>Export accounts…</button>
      <button onclick={() => open('import')}>Import accounts…</button>
      <button onclick={startQrScan}>Scan from desktop…</button>
    </div>
  {:else if mode === 'qr-scanning'}
    <p class="note">
      Point the camera at the code on your desktop and keep it steady — it
      cycles through several frames for a large account set.
      {#if qrProgress}Captured {qrProgress.have} of {qrProgress.total}.{/if}
    </p>
    <div class="row transfer-actions">
      <button onclick={cancelQrScan}>Cancel</button>
    </div>
  {:else if mode === 'qr-passphrase'}
    <p class="note">Scan complete. Enter the passphrase it was sealed under.</p>
    <label class="field">Passphrase
      <input type="password" autocomplete="current-password" bind:value={passphrase} />
    </label>
    <div class="row transfer-actions">
      <button class="primary" disabled={busy} onclick={doQrFinish}>
        {busy ? 'Working…' : 'Import'}
      </button>
      <button disabled={busy} onclick={() => open(null)}>Cancel</button>
    </div>
  {:else if mode === 'export'}
    <label class="field">Passphrase
      <input type="password" autocomplete="new-password" bind:value={passphrase} />
    </label>
    <label class="field">Confirm passphrase
      <input type="password" autocomplete="new-password" bind:value={confirm} />
    </label>
    <div class="row transfer-actions">
      <button class="primary" disabled={busy} onclick={doExport}>
        {busy ? 'Working…' : 'Choose where to save'}
      </button>
      <button disabled={busy} onclick={() => open(null)}>Cancel</button>
    </div>
  {:else}
    <label class="field">Passphrase
      <input type="password" autocomplete="current-password" bind:value={passphrase} />
    </label>
    <div class="row transfer-actions">
      <button class="primary" disabled={busy} onclick={doImport}>
        {busy ? 'Working…' : 'Choose the export file'}
      </button>
      <button disabled={busy} onclick={() => open(null)}>Cancel</button>
    </div>
  {/if}
  {#if error}
    <p class="test bad">{error}</p>
  {/if}
  {#if doneMsg}
    <p class="test good">{doneMsg}</p>
  {/if}
  {#if summary}
    <div class="import-summary">
      {#if summary.added.length > 0}
        <p class="test good">Added: {summary.added.join(', ')}</p>
      {/if}
      {#if summary.updated.length > 0}
        <p class="note">Updated: {summary.updated.join(', ')}</p>
      {/if}
      {#if summary.needsSignIn.length > 0}
        <p class="note">Needs sign-in: {summary.needsSignIn.join(', ')} — sign in from the account list above.</p>
      {/if}
      {#each summary.couldNotStore as failure (failure.key)}
        <p class="test bad">Could not store {failure.key}: {failure.reason}. The account was not added.</p>
      {/each}
      {#if summary.added.length + summary.updated.length + summary.needsSignIn.length + summary.couldNotStore.length === 0}
        <p class="note">The file held no accounts.</p>
      {/if}
    </div>
  {/if}
</section>
