// Playwright test: loads the Vite-bundled harness in a real headless browser
// and asserts the worker path works end to end. Run via `make test-browser`
// (or `npx playwright test`), which starts the Vite server automatically.

import { expect, test } from '@playwright/test'

test('createMonty runs feeds in a browser Web Worker', async ({ page }) => {
  await page.goto('/')
  await page.waitForFunction(() => window.__results !== undefined || window.__error !== undefined, null, {
    timeout: 30_000,
  })

  const error = await page.evaluate(() => window.__error)
  expect(error, error).toBeUndefined()

  const results = await page.evaluate(() => window.__results)
  expect(results).toMatchObject({
    add: 3, // basic feed via the worker
    ext: 5, // external function round-trip over postMessage
    watchdog: 'MontyCrashedError', // Worker.terminate() hard-killed the runaway turn
    recovered: 4, // the pool replaced the killed worker
    crossOriginIsolated: false, // no SharedArrayBuffer / COOP+COEP required
  })
})

declare global {
  interface Window {
    __results?: Record<string, unknown>
    __error?: string
  }
}
