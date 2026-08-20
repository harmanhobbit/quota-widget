import { describe, it, expect, vi, afterEach } from 'vitest';
import { runCredentialTest, CREDENTIAL_TEST_TIMEOUT_MS } from './credentialTest.js';

// The shared Android direct-HTTPS credential-test lifecycle (issue #135). These
// exercise it against a stubbed host IPC so every success and failure path is
// covered without an emulator: OpenRouter success, invalid credentials, a
// bounded timeout, a transport failure, a malformed response, a storage failure
// before the request, and the pending-state-cleanup contract.

const ok = () => Promise.resolve();

// A host whose IPC all succeeds, with a `testProvider` the caller supplies.
function host(over = {}) {
  return {
    setConfig: over.setConfig ?? vi.fn(ok),
    setSecret: over.setSecret ?? vi.fn(ok),
    testProvider: over.testProvider ?? vi.fn(() => Promise.resolve(snapshot())),
  };
}

// A snapshot as `test_provider` returns it. `error` is a serialized
// FetchError: { kind, detail } with the PascalCase tags quota-core emits.
function snapshot(error = null) {
  return { provider_id: 'openrouter', provider_name: 'OpenRouter', windows: [], credits: null, error };
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('runCredentialTest', () => {
  it('OpenRouter success stores the key, clears the input, and reports ok', async () => {
    const h = host();
    const r = await runCredentialTest({
      id: 'openrouter',
      pastedSecret: 'sk-or-good',
      snapshotConfig: {},
      host: h,
    });
    expect(r).toMatchObject({ ok: true, storedSecret: true, clearInput: true });
    expect(h.setSecret).toHaveBeenCalledWith('openrouter', 'sk-or-good');
    expect(h.testProvider).toHaveBeenCalledOnce();
  });

  it('invalid credentials surface as an auth failure and keep the input', async () => {
    const h = host({
      testProvider: () => Promise.resolve(snapshot({ kind: 'AuthExpired', detail: 'key rejected' })),
    });
    const r = await runCredentialTest({ id: 'openrouter', pastedSecret: 'sk-or-bad', snapshotConfig: {}, host: h });
    expect(r.ok).toBe(false);
    expect(r.category).toBe('auth');
    expect(r.msg).toBe('key rejected');
    expect(r.clearInput).toBe(false);
  });

  it('a hung request terminates at the bound instead of waiting forever', async () => {
    vi.useFakeTimers();
    const h = host({ testProvider: () => new Promise(() => {}) }); // never resolves
    const p = runCredentialTest({ id: 'openrouter', pastedSecret: 'sk', snapshotConfig: {}, host: h });
    await vi.advanceTimersByTimeAsync(CREDENTIAL_TEST_TIMEOUT_MS);
    const r = await p;
    expect(r.ok).toBe(false);
    expect(r.category).toBe('timeout');
    expect(r.clearInput).toBe(false);
  });

  it('a transport failure surfaces as a network failure', async () => {
    const h = host({ testProvider: () => Promise.reject(new Error('connection refused')) });
    const r = await runCredentialTest({ id: 'openrouter', pastedSecret: 'sk', snapshotConfig: {}, host: h });
    expect(r.ok).toBe(false);
    expect(r.category).toBe('network');
    expect(r.msg).toContain('connection refused');
  });

  it('a malformed response surfaces as a parse failure', async () => {
    const h = host({
      testProvider: () => Promise.resolve(snapshot({ kind: 'Parse', detail: 'unexpected JSON' })),
    });
    const r = await runCredentialTest({ id: 'openrouter', pastedSecret: 'sk', snapshotConfig: {}, host: h });
    expect(r.ok).toBe(false);
    expect(r.category).toBe('parse');
  });

  it('storage failure before the request is a storage failure and never hits the provider', async () => {
    const testProvider = vi.fn(() => Promise.resolve(snapshot()));
    const h = host({ setSecret: vi.fn(() => Promise.reject(new Error('keystore locked'))), testProvider });
    const r = await runCredentialTest({ id: 'openrouter', pastedSecret: 'sk', snapshotConfig: {}, host: h });
    expect(r.ok).toBe(false);
    expect(r.category).toBe('storage');
    expect(r.storedSecret).toBe(false); // a failed store must not imply acceptance
    expect(r.clearInput).toBe(false); // pasted key is preserved for retry
    expect(testProvider).not.toHaveBeenCalled(); // request never attempted
  });

  it('a failed config save before the request is also a storage failure', async () => {
    const testProvider = vi.fn(() => Promise.resolve(snapshot()));
    const h = host({ setConfig: vi.fn(() => Promise.reject(new Error('disk full'))), testProvider });
    const r = await runCredentialTest({ id: 'openrouter', pastedSecret: 'sk', snapshotConfig: {}, host: h });
    expect(r.category).toBe('storage');
    expect(testProvider).not.toHaveBeenCalled();
  });

  // The pending-state-cleanup contract: every path — success or any failure —
  // resolves to a terminal result carrying a boolean `ok` and no `pending`, so
  // a caller that swaps its `{ pending: true }` marker for this can never be
  // left showing `testing…`.
  it('every path resolves to a terminal, non-pending result', async () => {
    const cases = [
      host(), // success
      host({ testProvider: () => Promise.resolve(snapshot({ kind: 'AuthExpired', detail: 'x' })) }),
      host({ testProvider: () => Promise.reject(new Error('boom')) }),
      host({ setSecret: () => Promise.reject(new Error('nope')) }),
    ];
    for (const h of cases) {
      const r = await runCredentialTest({ id: 'openrouter', pastedSecret: 'sk', snapshotConfig: {}, host: h });
      expect(typeof r.ok).toBe('boolean');
      expect(r).not.toHaveProperty('pending');
      expect(typeof r.msg).toBe('string');
    }
  });
});
