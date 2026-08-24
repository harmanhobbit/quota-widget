// Tests for the desktop→phone QR scan flow (issue #156). The host is
// injected, so the camera and the Tauri commands are plain stubs here; a real
// Android build exercises the actual camera and Keystore.
import { describe, it, expect } from 'vitest';
import { runQrScan, finishQrImport } from './qrImport.js';

const REPORT = {
  accounts: {
    'openrouter#1': { outcome: 'added' },
    claude: { outcome: 'needs_onboarding' },
  },
};

describe('runQrScan', () => {
  const host = (over = {}) => ({
    qrCheckPermissions: async () => 'granted',
    qrRequestPermissions: async () => 'granted',
    qrScanReset: async () => {},
    qrScan: async () => ({ content: 'frame-1', format: 'QR_CODE' }),
    qrScanFrame: async () => ({ have: 1, total: 1, complete: true }),
    ...over,
  });

  it('resolves cancelled without scanning when permission is denied', async () => {
    let scanned = 0;
    const h = host({
      qrCheckPermissions: async () => 'denied',
      qrRequestPermissions: async () => 'denied',
      qrScan: async () => { scanned++; return { content: 'x' }; },
    });
    const r = await runQrScan({ host: h });
    expect(r.status).toBe('cancelled');
    expect(scanned).toBe(0);
  });

  it('requests permission when not already granted', async () => {
    let requested = 0;
    const h = host({
      qrCheckPermissions: async () => 'prompt',
      qrRequestPermissions: async () => { requested++; return 'granted'; },
    });
    const r = await runQrScan({ host: h });
    expect(requested).toBe(1);
    expect(r.status).toBe('complete');
  });

  it('loops scan→frame until the collector reports complete, reporting progress', async () => {
    const frames = ['a', 'b', 'c'];
    let i = 0;
    const progress = [];
    const h = host({
      qrScan: async () => ({ content: frames[i++] }),
      qrScanFrame: async (text) => {
        const have = frames.indexOf(text) + 1;
        return { have, total: frames.length, complete: have === frames.length };
      },
    });
    const r = await runQrScan({ host: h, onProgress: (s) => progress.push(s) });
    expect(r.status).toBe('complete');
    expect(i).toBe(3);
    expect(progress.map((s) => s.have)).toEqual([1, 2, 3]);
    expect(progress.at(-1).complete).toBe(true);
  });

  it('a cancelled camera view resolves to cancelled, not an error', async () => {
    let framed = 0;
    const h = host({
      qrScan: async () => { throw new Error('scan cancelled'); },
      qrScanFrame: async () => { framed++; return { have: 0, total: 0, complete: false }; },
    });
    const r = await runQrScan({ host: h });
    expect(r.status).toBe('cancelled');
    expect(framed).toBe(0);
  });

  it('a scan-frame command failure surfaces as failed', async () => {
    const h = host({
      qrScanFrame: async () => { throw new Error('scan state lost'); },
    });
    const r = await runQrScan({ host: h });
    expect(r).toEqual({ status: 'failed', msg: 'scan state lost' });
  });

  it('resets the collector before the first scan', async () => {
    let resetBeforeScan = false;
    let reset = false;
    const h = host({
      qrScanReset: async () => { reset = true; },
      qrScan: async () => { resetBeforeScan = reset; return { content: 'x' }; },
    });
    await runQrScan({ host: h });
    expect(resetBeforeScan).toBe(true);
  });
});

describe('finishQrImport', () => {
  it('calls the host command and summarizes the report', async () => {
    let got;
    const host = { qrScanFinish: async (passphrase) => { got = passphrase; return REPORT; } };
    const r = await finishQrImport({ passphrase: 'pw', host });
    expect(r.status).toBe('done');
    expect(got).toBe('pw');
    expect(r.summary.added).toEqual(['openrouter#1']);
    expect(r.summary.needsSignIn).toEqual(['claude']);
  });

  it('a refused bundle (wrong passphrase or incomplete scan) surfaces the error', async () => {
    const host = { qrScanFinish: async () => { throw new Error('the scan is not complete yet'); } };
    const r = await finishQrImport({ passphrase: 'pw', host });
    expect(r).toEqual({ status: 'failed', msg: 'the scan is not complete yet' });
  });
});
