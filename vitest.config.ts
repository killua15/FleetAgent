import { defineConfig } from 'vitest/config';

// Node, not jsdom: everything worth testing here is a pure decision — a diff,
// a layout, a severity threshold, a filter. Booting a DOM for those buys
// nothing and costs seconds on every run, which is what stops people running
// tests at all. A component test that genuinely needs a DOM can opt in with a
// `// @vitest-environment jsdom` comment.
export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
  },
});
