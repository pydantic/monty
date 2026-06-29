import { defineConfig } from '@playwright/test'

// Browser verification of the `@pydantic/monty/wasm` worker path. Starts the
// Vite dev server (serving `browser-test/`) automatically, then runs the spec
// in headless Chromium. Locally: `make test-browser`.
export default defineConfig({
  testDir: './browser-test',
  testMatch: '**/*.spec.ts',
  timeout: 60_000,
  fullyParallel: true,
  use: { baseURL: 'http://localhost:5179' },
  webServer: {
    command: 'npx vite',
    cwd: 'browser-test',
    url: 'http://localhost:5179',
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
  projects: [{ name: 'chromium', use: { browserName: 'chromium' } }],
})
