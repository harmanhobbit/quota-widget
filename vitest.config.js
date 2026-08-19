import { defineConfig } from 'vitest/config';

// Deliberately separate from vite.config.js: the vitest suite only unit-tests
// pure modules (period.js) under Node, so it must not pull in the Svelte
// plugin or the app build's `define` globals.
export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/**/*.test.js'],
  },
});
