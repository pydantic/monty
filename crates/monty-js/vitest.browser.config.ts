import { defineConfig } from 'vitest/config'

export default defineConfig({
  optimizeDeps: { exclude: ['@pydantic/monty'] },
  server: {
    port: 5179,
    strictPort: true,
  },
  test: {
    include: ['browser-test/*.spec.ts'],
    testTimeout: 60_000,
    browser: {
      enabled: true,
      provider: 'playwright',
      headless: true,
      instances: [{ browser: 'chromium' }],
    },
  },
})
