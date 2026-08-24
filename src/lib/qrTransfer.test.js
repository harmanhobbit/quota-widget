// Tests for the desktop side of the QR transfer flow (issue #156). The host
// is injected, so the Tauri command is a plain stub here.
import { describe, it, expect } from 'vitest';
import { runQrTransfer } from './qrTransfer.js';

describe('runQrTransfer', () => {
  const host = (over = {}) => ({
    qrTransferFrames: async () => ['<svg>frame-1</svg>'],
    ...over,
  });

  it('refuses an empty passphrase before calling the host', async () => {
    let calls = 0;
    const h = host({ qrTransferFrames: async () => { calls++; return []; } });
    const r = await runQrTransfer({ passphrase: '', confirm: '', host: h });
    expect(r.status).toBe('failed');
    expect(r.msg).toMatch(/passphrase/i);
    expect(calls).toBe(0);
  });

  it('refuses a mismatched confirmation before calling the host', async () => {
    let calls = 0;
    const h = host({ qrTransferFrames: async () => { calls++; return []; } });
    const r = await runQrTransfer({ passphrase: 'one', confirm: 'two', host: h });
    expect(r.status).toBe('failed');
    expect(r.msg).toMatch(/do not match/);
    expect(calls).toBe(0);
  });

  it('passes the passphrase to the command and returns the frames', async () => {
    let got;
    const h = host({
      qrTransferFrames: async (passphrase) => { got = passphrase; return ['<svg>a</svg>', '<svg>b</svg>']; },
    });
    const r = await runQrTransfer({ passphrase: 'pw', confirm: 'pw', host: h });
    expect(r.status).toBe('ready');
    expect(got).toBe('pw');
    expect(r.frames).toEqual(['<svg>a</svg>', '<svg>b</svg>']);
  });

  it('an oversized transfer surfaces the host error', async () => {
    const h = host({
      qrTransferFrames: async () => {
        throw new Error(
          'this transfer needs 25 QR codes, but only 20 are supported — pair over the local ' +
            'network or export a credentials file instead of scanning this many accounts at once',
        );
      },
    });
    const r = await runQrTransfer({ passphrase: 'pw', confirm: 'pw', host: h });
    expect(r.status).toBe('failed');
    expect(r.msg).toMatch(/export a credentials file/);
  });
});
