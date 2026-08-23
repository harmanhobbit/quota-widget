import { describe, it, expect, vi } from 'vitest';
import { openExternal } from './browserHandoff.js';

describe('openExternal', () => {
  it('calls the opener with the URL', async () => {
    const opener = vi.fn().mockResolvedValue(undefined);
    await openExternal('https://example.com/authorize', { opener });
    expect(opener).toHaveBeenCalledWith('https://example.com/authorize');
  });

  it('throws when no opener is provided', async () => {
    await expect(openExternal('https://example.com', {})).rejects.toThrow(
      'no external browser opener',
    );
    await expect(openExternal('https://example.com')).rejects.toThrow(
      'no external browser opener',
    );
  });

  it('propagates the opener rejection', async () => {
    const opener = vi.fn().mockRejectedValue(new Error('opener plugin unavailable'));
    await expect(openExternal('https://example.com', { opener })).rejects.toThrow(
      'opener plugin unavailable',
    );
  });
});
