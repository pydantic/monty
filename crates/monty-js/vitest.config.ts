import { resolve } from 'node:path'

import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['__test__/*.spec.ts'],
    exclude: ['__test__/ava-compat.ts'],
    testTimeout: 120_000,
    hookTimeout: 120_000,
    fileParallelism: false,
  },
  resolve: {
    alias: [{ find: 'ava', replacement: resolve(import.meta.dirname, '__test__/ava-compat.ts') }],
  },
})
