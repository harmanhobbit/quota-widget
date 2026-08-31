// Tests for the Android LAN pairing flow (issue #155). The host is injected
// by the shared flow module desktop also uses (`src/lib/lanPairing.js`), so
// there is nothing to stub here beyond the event payload the Rust receiver
// emits — a real Android build exercises the actual sockets and Keystore.
import { describe, it, expect } from 'vitest';
import { handleLanPairingResult } from './lanPairing.js';

const REPORT = {
  accounts: {
    'openrouter#1': { outcome: 'added' },
    elevenlabs: { outcome: 'updated' },
    claude: { outcome: 'needs_onboarding' },
    deepseek: { outcome: 'could_not_store', reason: 'keystore locked' },
  },
};

describe('handleLanPairingResult', () => {
  it('folds an applied report into the named-list summary the imports use', () => {
    const r = handleLanPairingResult({ ok: true, report: REPORT });
    expect(r.status).toBe('applied');
    expect(r.summary.added).toEqual(['openrouter#1']);
    expect(r.summary.updated).toEqual(['elevenlabs']);
    expect(r.summary.needsSignIn).toEqual(['claude']);
    expect(r.summary.couldNotStore).toEqual([{ key: 'deepseek', reason: 'keystore locked' }]);
    // The accounts the transfer named, in bundle order — the caller reloads
    // exactly these rather than the whole app.
    expect(r.keys).toEqual(['openrouter#1', 'elevenlabs', 'claude', 'deepseek']);
  });

  it('keeps a failure a failure, whatever the shape it arrived in', () => {
    expect(
      handleLanPairingResult({
        ok: false,
        error: 'The transfer stalled — nothing was changed.',
      }),
    ).toEqual({ status: 'failed', msg: 'The transfer stalled — nothing was changed.' });
    // A null/garbage payload reads as a failure, never as an applied transfer.
    expect(handleLanPairingResult(null).status).toBe('failed');
    expect(handleLanPairingResult({}).status).toBe('failed');
  });
});
