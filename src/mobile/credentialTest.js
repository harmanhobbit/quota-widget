// The shared asynchronous credential-test lifecycle for every Android
// direct-HTTPS (pasted-key) provider — issue #135. One code path so no
// provider can diverge in how it cleans up pending/error state.
//
// Guarantees, regardless of how the host IPC behaves:
//   1. The pasted key is stored BEFORE the provider request. A storage
//      failure is reported as a storage failure and the request is never
//      attempted, so a failed test can never look like the key was accepted
//      (the #133 secure-storage contract: storing a credential is not the same
//      as the provider accepting it).
//   2. The provider request is bounded on the JS side. A hung or very slow
//      network call rejects with a `timeout` result instead of leaving the row
//      on `testing…` forever — no unbounded JavaScript-side wait.
//   3. The function never throws and always resolves to a terminal result
//      ({ ok, category, msg, … }). The caller swaps its `pending` marker for
//      this, so the UI is never stuck pending on any success or failure path.
//   4. The pasted key is only consumed on full success (`clearInput`), so a
//      failed test always leaves the value in the form for correction/retry.

// Bound for a single provider request. Long enough to tolerate a slow mobile
// network, short enough that a dead connection surfaces as an actionable
// timeout rather than an apparent hang.
export const CREDENTIAL_TEST_TIMEOUT_MS = 15000;

// A private sentinel so a real request rejection (transport error) is never
// mistaken for the timeout firing, and vice versa.
const TIMEOUT = Symbol('credential-test-timeout');

function withTimeout(promise, ms) {
  let timer;
  const bound = new Promise((_, reject) => {
    timer = setTimeout(() => reject(TIMEOUT), ms);
  });
  return Promise.race([promise, bound]).finally(() => clearTimeout(timer));
}

// Backend `FetchError.kind` (PascalCase — see crates/quota-core/src/model.rs)
// mapped to the user-facing failure category, preserving the storage / timeout
// / network / provider-status / response-parse distinction the issue asks for.
function categorize(kind) {
  switch (kind) {
    case 'AuthExpired':
      return 'auth';
    case 'Unavailable':
      return 'storage';
    case 'Network':
      return 'network';
    case 'Parse':
      return 'parse';
    case 'NotConfigured':
      return 'config';
    default:
      return 'provider';
  }
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
 * Run the credential-test lifecycle for one account.
 *
 * @param {object}   opts
 * @param {string}   opts.id             Account id (also the secret store key).
 * @param {string}   [opts.pastedSecret] Value currently in the form, if any.
 * @param {object}   [opts.snapshotConfig] Plain (already `$state.snapshot`-ed)
 *                                        config to persist before testing, so
 *                                        provider-specific edits (account id,
 *                                        balance URL, budget) are in effect.
 * @param {string}   [opts.secretLabel]  Human label for storage-failure copy.
 * @param {object}   opts.host           Injected IPC: { setConfig, setSecret,
 *                                        testProvider }.
 * @param {number}   [opts.timeoutMs]    Per-request bound.
 * @returns {Promise<{ok: boolean, category: string|null, msg: string,
 *                     storedSecret: boolean, clearInput: boolean}>}
 *          Always a terminal result — never pending, never a throw.
 */
export async function runCredentialTest({
  id,
  pastedSecret,
  snapshotConfig,
  secretLabel = 'API key',
  host,
  timeoutMs = CREDENTIAL_TEST_TIMEOUT_MS,
}) {
  const { setConfig, setSecret, testProvider } = host;

  // 1. Persist pending config edits and store the pasted key — both before the
  //    request. Either failing is a storage failure: the request is never
  //    attempted, the pasted key is kept, and the result says storage, not a
  //    provider/credential rejection.
  let storedSecret = false;
  try {
    if (snapshotConfig !== undefined) await setConfig(snapshotConfig);
    if (pastedSecret) {
      await setSecret(id, pastedSecret);
      storedSecret = true;
    }
  } catch (e) {
    return {
      ok: false,
      category: 'storage',
      msg: `Couldn't save the ${secretLabel} to secure storage: ${errText(e)}. The key was not tested — try again.`,
      storedSecret,
      clearInput: false,
    };
  }

  // 2. Run the provider request under a bounded timeout so a hung call cannot
  //    leave the row on `testing…`.
  let snap;
  try {
    snap = await withTimeout(testProvider(id), timeoutMs);
  } catch (e) {
    if (e === TIMEOUT) {
      return {
        ok: false,
        category: 'timeout',
        msg: `Timed out after ${Math.round(timeoutMs / 1000)}s waiting for the provider. Check your connection and try again.`,
        storedSecret,
        clearInput: false,
      };
    }
    return {
      ok: false,
      category: 'network',
      msg: `Couldn't reach the provider: ${errText(e)}.`,
      storedSecret,
      clearInput: false,
    };
  }

  // 3. A returned snapshot can still carry its own typed failure
  //    (auth / network / parse / unavailable / not-configured).
  if (snap && snap.error) {
    return {
      ok: false,
      category: categorize(snap.error.kind),
      msg: snap.error.detail || 'The provider reported an error.',
      storedSecret,
      clearInput: false,
    };
  }

  // 4. Success — the stored key is confirmed working; the caller may clear the
  //    input now that it is safely persisted.
  return { ok: true, category: null, msg: 'ok', storedSecret, clearInput: true };
}
