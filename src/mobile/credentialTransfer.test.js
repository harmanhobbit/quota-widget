// Tests for the credential export/import flows (issue #152). The host is
// injected, so the system dialog and the Tauri commands are plain stubs here;
// a real Android build exercises the actual SAF dialog and Keystore.
import { describe, it, expect } from 'vitest';
import { summarizeImport, runExport, runImport } from './credentialTransfer.js';

const REPORT = {
  accounts: {
    'openrouter#1': { outcome: 'added' },
    elevenlabs: { outcome: 'updated' },
    claude: { outcome: 'needs_onboarding' },
    deepseek: { outcome: 'could_not_store', reason: 'keystore locked' },
  },
};

describe('summarizeImport', () => {
  it('groups every outcome kind, preserving bundle order', () => {
    const s = summarizeImport(REPORT);
    expect(s.added).toEqual(['openrouter#1']);
    expect(s.updated).toEqual(['elevenlabs']);
    expect(s.needsSignIn).toEqual(['claude']);
    expect(s.couldNotStore).toEqual([{ key: 'deepseek', reason: 'keystore locked' }]);
  });

  it('treats a missing report as an empty one', () => {
    expect(summarizeImport(null)).toEqual({ added: [], updated: [], needsSignIn: [], couldNotStore: [] });
    expect(summarizeImport({})).toEqual({ added: [], updated: [], needsSignIn: [], couldNotStore: [] });
  });

  it('gives a could-not-store without a reason an honest placeholder', () => {
    const s = summarizeImport({ accounts: { x: { outcome: 'could_not_store' } } });
    expect(s.couldNotStore[0].reason).toBe('unknown reason');
  });
});

describe('runExport', () => {
  const host = (over = {}) => ({
    pickExportDestination: async () => 'content://picked/export.qwb',
    exportCredentials: async () => {},
    ...over,
  });

  it('refuses an empty passphrase before any dialog or command', async () => {
    let calls = 0;
    const h = host({
      pickExportDestination: async () => { calls++; return 'x'; },
      exportCredentials: async () => { calls++; },
    });
    const r = await runExport({ passphrase: '', confirm: '', host: h });
    expect(r.status).toBe('failed');
    expect(r.msg).toMatch(/passphrase/i);
    expect(calls).toBe(0);
  });

  it('refuses a mismatched confirmation before any dialog or command', async () => {
    let calls = 0;
    const h = host({
      pickExportDestination: async () => { calls++; return 'x'; },
      exportCredentials: async () => { calls++; },
    });
    const r = await runExport({ passphrase: 'one', confirm: 'two', host: h });
    expect(r.status).toBe('failed');
    expect(r.msg).toMatch(/do not match/);
    expect(calls).toBe(0);
  });

  it('passes the picked destination and passphrase to the command', async () => {
    let got;
    const h = host({
      exportCredentials: async (destination, passphrase) => { got = { destination, passphrase }; },
    });
    const r = await runExport({ passphrase: 'pw', confirm: 'pw', host: h });
    expect(r.status).toBe('done');
    expect(got).toEqual({ destination: 'content://picked/export.qwb', passphrase: 'pw' });
  });

  it('a cancelled dialog is a quiet cancellation, not an error', async () => {
    let exported = 0;
    const h = host({
      pickExportDestination: async () => null,
      exportCredentials: async () => { exported++; },
    });
    const r = await runExport({ passphrase: 'pw', confirm: 'pw', host: h });
    expect(r.status).toBe('cancelled');
    expect(exported).toBe(0);
  });

  it('a failed command surfaces the error', async () => {
    const h = host({ exportCredentials: async () => { throw new Error('disk gone'); } });
    const r = await runExport({ passphrase: 'pw', confirm: 'pw', host: h });
    expect(r).toEqual({ status: 'failed', msg: 'disk gone' });
  });
});

describe('runImport', () => {
  const host = (over = {}) => ({
    pickImportSource: async () => 'content://picked/export.qwb',
    importCredentials: async () => REPORT,
    ...over,
  });

  it('passes the picked source and passphrase to the command and summarizes', async () => {
    let got;
    const h = host({
      importCredentials: async (source, passphrase) => { got = { source, passphrase }; return REPORT; },
    });
    const r = await runImport({ passphrase: 'pw', host: h });
    expect(r.status).toBe('done');
    expect(got).toEqual({ source: 'content://picked/export.qwb', passphrase: 'pw' });
    expect(r.summary.needsSignIn).toEqual(['claude']);
  });

  it('a cancelled picker is a quiet cancellation, not an error', async () => {
    let imported = 0;
    const h = host({
      pickImportSource: async () => null,
      importCredentials: async () => { imported++; },
    });
    const r = await runImport({ passphrase: 'pw', host: h });
    expect(r.status).toBe('cancelled');
    expect(imported).toBe(0);
  });

  it('a refused file (wrong passphrase or corrupt) surfaces the error', async () => {
    const h = host({
      importCredentials: async () => {
        throw new Error('wrong passphrase, or the sealed bundle was tampered with or truncated');
      },
    });
    const r = await runImport({ passphrase: 'wrong', host: h });
    expect(r.status).toBe('failed');
    expect(r.msg).toMatch(/wrong passphrase/);
  });
});
