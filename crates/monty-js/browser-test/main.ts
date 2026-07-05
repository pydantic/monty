// The browser-side harness. Imported by `index.html`, bundled by Vite, and
// driven by `browser.spec.ts` under Playwright. It exercises the four seams
// that only a real browser can verify: fetch + `compileStreaming` loading,
// `new URL(..., import.meta.url)` asset/worker resolution through a bundler, the
// DOM `Worker` (terminate watchdog included), and that none of it needs
// cross-origin isolation. Results are stashed on `window` for the spec to read.

import { createMonty } from '@pydantic/monty/wasm'

declare global {
  interface Window {
    __results?: Record<string, unknown>
    __error?: string
  }
}

async function main(): Promise<void> {
  const results: Record<string, unknown> = {}
  // proves we did NOT require COOP/COEP (the old napi-threads build did)
  results.crossOriginIsolated = globalThis.crossOriginIsolated

  // auto-loads the bundled wasm and runs in a real Web Worker
  const pool = await createMonty({ requestTimeoutMs: 2000 })

  const session = await pool.checkout()
  results.add = await session.feedRun('1 + 2')
  results.ext = await session.feedRun('add_ints(2, 3)', {
    externalLookup: { add_ints: (a: number, b: number) => a + b },
  })
  await session.close()

  // the watchdog hard-kills a runaway turn via Worker.terminate()
  const runaway = await pool.checkout()
  try {
    await runaway.feedRun('while True:\n    pass')
    results.watchdog = 'not killed'
  } catch (err) {
    results.watchdog = (err as { constructor: { name: string } }).constructor.name
  }
  await runaway.close()

  // the pool recovers and keeps serving
  const recovered = await pool.checkout()
  results.recovered = await recovered.feedRun('2 + 2')
  await recovered.close()

  await pool.close()
  window.__results = results
  document.getElementById('status')!.textContent = 'done'
}

main().catch((err: unknown) => {
  window.__error = String(err instanceof Error ? err.stack : err)
  document.getElementById('status')!.textContent = 'error'
})
