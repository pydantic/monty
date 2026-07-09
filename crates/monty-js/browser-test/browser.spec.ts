import { expect, test } from '@playwright/test'

test('Monty wasm runs in a browser', async ({ page }) => {
  const messages: string[] = []
  page.on('console', (message) => messages.push(message.text()))
  page.on('pageerror', (error) => messages.push(error.stack ?? error.message))

  await page.goto('/')
  await page
    .waitForFunction(() => window.__results !== undefined || window.__error !== undefined, null, {
      timeout: 30_000,
    })
    .catch((error: unknown) => {
      throw new Error(`${String(error)}\n${messages.join('\n')}`)
    })

  const error = await page.evaluate(() => window.__error)
  expect(error, error).toBeUndefined()

  const results = await page.evaluate(() => window.__results)
  expect(results).toMatchObject({
    add: 3,
    ext: 5,
    repl: 4,
    crossOriginIsolated: true,
  })
})

declare global {
  interface Window {
    __results?: Record<string, unknown>
    __error?: string
  }
}
