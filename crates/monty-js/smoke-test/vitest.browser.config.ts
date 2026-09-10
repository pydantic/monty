import { defineConfig } from 'vitest/config'

export default defineConfig({
  optimizeDeps: { exclude: ['@pydantic/monty'] },
  test: {
    include: ['browser.test.ts'],
    testTimeout: 60_000,
    hookTimeout: 60_000,
    browser: {
      enabled: true,
      provider: 'playwright',
      headless: true,
      instances: [{ browser: 'chromium' }],
    },
  },
})
