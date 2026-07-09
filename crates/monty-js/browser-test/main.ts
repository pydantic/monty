// Browser-side harness imported by `index.html`, bundled by Vite, and driven
// by `browser.spec.ts` under Playwright. Results are stashed on `window` for
// the spec to read.

import { Monty, MontyRepl } from '../ts/wasm.ts'

declare global {
  interface Window {
    __results?: Record<string, unknown>
    __error?: string
  }
}

function main(): void {
  const results: Record<string, unknown> = {}
  results.crossOriginIsolated = globalThis.crossOriginIsolated

  const add = new Monty('1 + 2')
  results.add = add.run()

  const ext = new Monty('add_ints(2, 3)')
  results.ext = ext.run({ externalLookup: { add_ints: (a: number, b: number) => a + b } })

  const repl = new MontyRepl()
  repl.feed('x = 2')
  results.repl = repl.feed('x + 2')

  window.__results = results
  document.getElementById('status')!.textContent = 'done'
}

try {
  main()
} catch (err: unknown) {
  window.__error = String(err instanceof Error ? (err.stack ?? err.message) : err)
  document.getElementById('status')!.textContent = 'error'
}
