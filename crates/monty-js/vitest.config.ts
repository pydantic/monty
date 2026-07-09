import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['__test__/*.spec.ts'],
    testTimeout: 120_000,
    hookTimeout: 120_000,
    fileParallelism: false,
  },
})
