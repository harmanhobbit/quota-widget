<script>
  // LAN device pairing on Android (issues #154/#155): the live transport for
  // the same credential bundle the export file and QR code move (ADR-0008).
  // The flow logic and session rules are the ones desktop's Settings uses —
  // `runLanSend`/`runLanReceiveWait` from the shared `lanPairing.js`, against
  // the same `lan_pairing_*` commands both shells register — so both
  // directions work between a desktop and this phone with one 6-digit code
  // each. The code never crosses the network; it drives the PAKE and derives
  // the transfer's key. OAuth and cookie accounts arrive awaiting sign-in
  // (provider onboarding), pasted keys land in the Keystore, and a failed or
  // cancelled transfer changes nothing.
  import { onMount } from 'svelte';
  import { runLanSend, runLanReceiveWait } from '../lib/lanPairing.js';
  import { handleLanPairingResult } from './lanPairing.js';
  import {
    lanPairingAddresses,
    lanPairingGenerateCode,
    lanPairingSend,
    lanPairingReceiveStart,
    lanPairingCancel,
    listen,
  } from '../lib/host.js';

  let {
    // Called after a received transfer actually landed accounts, so the
    // parent can re-read the configuration and refresh the usage list.
    onImported,
  } = $props();

  let pairMode = $state(''); // '', 'send', 'receive'
  let pairCode = $state('');
  // The address the user typed on the send side; the receive side fetches
  // this device's own addresses into `pairAddresses` instead.
  let pairAddress = $state('');
  // This device's address(es) as ready-to-type bare IPv4 strings — the
  // shell's fixed pairing port is appended when the sender dials, so the
  // user never types one; fetched when the receive flow opens, and only
  // shown on the receive side.
  let pairAddresses = $state([]);
  let pairBusy = $state(false);
  let pairWaiting = $state(false);
  let pairMessage = $state('');
  // The receiver's four-way summary, same shape and wording as this phone's
  // file/QR import. Cleared on a new attempt, not on leaving — the user may
  // have stepped away from a failed attempt's explanation.
  let pairSummary = $state(null);

  function openPairing(mode) {
    pairMode = mode;
    pairCode = '';
    pairMessage = '';
    pairSummary = null;
    pairWaiting = false;
    pairBusy = false;
    if (mode === 'receive') {
      lanPairingAddresses()
        .then((addrs) => (pairAddresses = Array.isArray(addrs) ? addrs : []))
        .catch(() => (pairAddresses = []));
    }
  }

  function closePairing() {
    // A send in flight or an armed session: stop it at the shell too, so a
    // peer that is stalling the exchange loses its socket and the port is
    // freed. Nothing armed makes the command a harmless no-op.
    if (pairBusy || pairWaiting) void lanPairingCancel().catch(() => {});
    pairMode = '';
    pairCode = '';
    pairMessage = '';
    pairSummary = null;
    pairWaiting = false;
    pairBusy = false;
  }

  async function generateCode() {
    try {
      pairCode = await lanPairingGenerateCode();
    } catch (e) {
      pairMessage = `Could not generate a code: ${e}`;
    }
  }

  async function sendAccounts() {
    pairMessage = '';
    pairSummary = null;
    pairBusy = true;
    const r = await runLanSend({ code: pairCode, address: pairAddress, host: { lanPairingSend } });
    pairBusy = false;
    if (r.status === 'failed') {
      pairMessage = r.msg;
      return;
    }
    // One transfer per code, as on the receiver.
    pairCode = '';
    pairMessage = 'Accounts sent — the other device has them and will show a summary.';
  }

  async function waitToReceive() {
    pairMessage = '';
    pairSummary = null;
    pairBusy = true;
    const r = await runLanReceiveWait({ code: pairCode, host: { lanPairingReceiveStart } });
    pairBusy = false;
    if (r.status === 'failed') {
      pairMessage = r.msg;
      return;
    }
    pairWaiting = true;
  }

  async function cancelReceive() {
    try {
      await lanPairingCancel();
    } catch {
      // Nothing armed: nothing to undo.
    }
    pairWaiting = false;
    pairCode = '';
    pairMessage = 'Cancelled — nothing was transferred.';
  }

  // The armed session's one outcome. On success this is the receiver's
  // import summary, and the accounts it names are reloaded exactly as the
  // file/QR imports reload them.
  function onLanPairingResult(payload) {
    pairWaiting = false;
    const r = handleLanPairingResult(payload);
    if (r.status === 'applied') {
      pairMessage = '';
      pairSummary = r.summary;
      // Only accounts that landed are worth a reload: a transfer whose every
      // account failed to store changed nothing on this phone.
      const landed =
        r.summary.added.length + r.summary.updated.length + r.summary.needsSignIn.length > 0;
      if (landed) void onImported?.();
    } else {
      pairMessage = r.msg;
    }
    // Single-use: the armed code is spent whatever the outcome.
    pairCode = '';
  }

  onMount(() => {
    const unlisten = [];
    listen('lan-pairing', (e) => onLanPairingResult(e.payload)).then((u) => unlisten.push(u));
    return () => {
      unlisten.forEach((u) => u());
      // Leaving Settings while a session is armed stops it: an armed code
      // must not keep a listener running with nobody watching, and a
      // re-opened panel always starts fresh.
      if (pairBusy || pairWaiting) void lanPairingCancel().catch(() => {});
    };
  });
</script>

<section class="lan-pairing">
  <h2>Pair over the network</h2>
  {#if !pairMode}
    <div class="row transfer-actions">
      <button onclick={() => openPairing('send')}>Send to another device…</button>
      <button onclick={() => openPairing('receive')}>Receive on this phone…</button>
    </div>
    <p class="note">
      Move every account to another device over your local network — no server
      in between. Both devices enter the same 6-digit code. Pasted keys work
      immediately; OAuth and cookie accounts (Claude, Codex, Hermes Portal)
      arrive awaiting sign-in.
    </p>
  {:else if pairMode === 'send'}
    <label class="field">Pairing code
      <input inputmode="numeric" maxlength="6" placeholder="6-digit code from the other device" bind:value={pairCode} />
    </label>
    <label class="field">Other device's address
      <input placeholder="e.g. 192.168.1.20" bind:value={pairAddress} />
    </label>
    <div class="row transfer-actions">
      <button class="primary" disabled={pairBusy} onclick={sendAccounts}>
        {pairBusy ? 'Sending…' : 'Send accounts'}
      </button>
      <!-- Enabled while sending on purpose: a stalled receiver must not
           leave the user watching a spinner they cannot leave. Cancel
           aborts the exchange at the shell and resets the panel. -->
      <button onclick={closePairing}>Cancel</button>
    </div>
    <p class="note">The other device shows its address while it waits to receive.</p>
  {:else}
    <div class="field">
      <span>This phone's address</span>
      {#if pairAddresses.length}
        <p class="device-code">{pairAddresses[0]}</p>
      {:else}
        <p class="note">
          Could not work out this phone's Wi-Fi address — find it in Android's
          network settings and type it on the other device.
        </p>
      {/if}
    </div>
    <div class="field">
      <span>Pairing code</span>
      <div class="row transfer-actions">
        <input inputmode="numeric" maxlength="6" placeholder="6-digit code" bind:value={pairCode} />
        <button disabled={pairWaiting} onclick={generateCode}>Generate</button>
      </div>
    </div>
    {#if !pairWaiting}
      <div class="row transfer-actions">
        <button class="primary" disabled={pairBusy} onclick={waitToReceive}>
          {pairBusy ? 'Arming…' : 'Wait for the sender'}
        </button>
        <button disabled={pairBusy} onclick={closePairing}>Cancel</button>
      </div>
      <p class="note">
        Enter the same code on the sending device, which then needs this
        phone's address above. A code arms one transfer attempt only — success
        or not, generate a fresh one for the next transfer.
      </p>
    {:else}
      <p class="note">Waiting for the other device to send…</p>
      <div class="row transfer-actions">
        <button onclick={cancelReceive}>Cancel</button>
      </div>
    {/if}
  {/if}
  {#if pairMessage}<p class="note">{pairMessage}</p>{/if}
  {#if pairSummary}
    <div class="import-summary">
      {#if pairSummary.added.length > 0}
        <p class="test good">Added: {pairSummary.added.join(', ')}</p>
      {/if}
      {#if pairSummary.updated.length > 0}
        <p class="note">Updated: {pairSummary.updated.join(', ')}</p>
      {/if}
      {#if pairSummary.needsSignIn.length > 0}
        <p class="note">Needs sign-in: {pairSummary.needsSignIn.join(', ')} — sign in from the account list above.</p>
      {/if}
      {#each pairSummary.couldNotStore as failure (failure.key)}
        <p class="test bad">Could not store {failure.key}: {failure.reason}. The account was not added.</p>
      {/each}
      {#if pairSummary.added.length + pairSummary.updated.length + pairSummary.needsSignIn.length + pairSummary.couldNotStore.length === 0}
        <p class="note">The other device sent no accounts.</p>
      {/if}
    </div>
  {/if}
</section>
