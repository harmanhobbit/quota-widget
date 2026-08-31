// Tests for the LAN pairing frontend flow (issue #154). The host is injected,
// so the Tauri commands are plain stubs here — the handshake and transfer
// themselves are proven at the quota_core::pairing seam in Rust.
import { describe, it, expect } from 'vitest';
import {
  runLanSend,
  runLanReceiveWait,
  handleLanResult,
  summarizeReport,
} from './lanPairing.js';

describe('runLanSend', () => {
  const host = (over = {}) => ({
    lanPairingSend: async () => {},
    ...over,
  });

  it('refuses an incomplete code before calling the host', async () => {
    let calls = 0;
    const h = host({ lanPairingSend: async () => { calls++; } });
    expect((await runLanSend({ code: '12345', address: '192.168.1.2', host: h })).status).toBe('failed');
    expect((await runLanSend({ code: '1234a6', address: '192.168.1.2', host: h })).status).toBe('failed');
    expect((await runLanSend({ code: '', address: '192.168.1.2', host: h })).status).toBe('failed');
    expect(calls).toBe(0);
  });

  it('refuses a missing address before calling the host', async () => {
    let calls = 0;
    const h = host({ lanPairingSend: async () => { calls++; } });
    const r = await runLanSend({ code: '123456', address: '   ', host: h });
    expect(r.status).toBe('failed');
    expect(r.msg).toMatch(/address/i);
    expect(calls).toBe(0);
  });

  it('passes the code and trimmed address to the command', async () => {
    let got;
    const h = host({ lanPairingSend: async (code, address) => { got = [code, address]; } });
    const r = await runLanSend({ code: '654321', address: ' 192.168.1.20 ', host: h });
    expect(r.status).toBe('sent');
    expect(got).toEqual(['654321', '192.168.1.20']);
  });

  it('a refused transfer (wrong code) surfaces the host error', async () => {
    const h = host({ lanPairingSend: async () => { throw new Error('The pairing code did not match. Nothing was transferred.'); } });
    const r = await runLanSend({ code: '111111', address: '192.168.1.20', host: h });
    expect(r.status).toBe('failed');
    expect(r.msg).toMatch(/did not match/);
  });
});

describe('runLanReceiveWait', () => {
  const host = (over = {}) => ({
    lanPairingReceiveStart: async () => {},
    ...over,
  });

  it('refuses an invalid code before arming the session', async () => {
    let calls = 0;
    const h = host({ lanPairingReceiveStart: async () => { calls++; } });
    const r = await runLanReceiveWait({ code: '12a456', host: h });
    expect(r.status).toBe('failed');
    expect(calls).toBe(0);
  });

  it('arms the session and reports waiting', async () => {
    let got;
    const h = host({ lanPairingReceiveStart: async (code) => { got = code; } });
    const r = await runLanReceiveWait({ code: '000000', host: h });
    expect(r.status).toBe('waiting');
    expect(got).toBe('000000');
  });

  it('an already-waiting session surfaces the host error', async () => {
    const h = host({ lanPairingReceiveStart: async () => { throw new Error('A pairing is already waiting on this device — cancel it first.'); } });
    const r = await runLanReceiveWait({ code: '123456', host: h });
    expect(r.status).toBe('failed');
    expect(r.msg).toMatch(/already waiting/);
  });
});

describe('handleLanResult', () => {
  it('a successful event becomes the four-way import summary', () => {
    const r = handleLanResult({
      ok: true,
      report: {
        accounts: {
          elevenlabs: { outcome: 'added' },
          'openrouter#1': { outcome: 'updated' },
          claude: { outcome: 'needs_onboarding' },
          deepseek: { outcome: 'could_not_store', reason: 'keystore locked' },
          venice: { outcome: 'added' },
        },
      },
    });
    expect(r.status).toBe('applied');
    expect(r.summary).toEqual({ added: 2, updated: 1, needs_onboarding: 1, could_not_store: 1 });
    expect(r.keys).toEqual(['elevenlabs', 'openrouter#1', 'claude', 'deepseek', 'venice']);
  });

  it('a failed event keeps the receiver message', () => {
    const r = handleLanResult({ ok: false, error: 'The pairing code did not match, or the transfer was tampered with. Nothing was changed.' });
    expect(r.status).toBe('failed');
    expect(r.msg).toMatch(/did not match/);
    expect(r.summary).toBeUndefined();
  });
});

describe('summarizeReport', () => {
  it('an empty bundle summarizes to all zeros', () => {
    expect(summarizeReport({ accounts: {} })).toEqual({
      added: 0,
      updated: 0,
      needs_onboarding: 0,
      could_not_store: 0,
    });
    expect(summarizeReport(undefined)).toEqual(summarizeReport({ accounts: {} }));
  });
});
