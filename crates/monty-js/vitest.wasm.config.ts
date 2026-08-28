import { defineConfig } from 'vitest/config'

// The wasm worker driven from node, with no browser: needs
// `dist/worker/monty_wasm_runtime.wasm`, so run `npm run build:wasm` first
// (`make test-wasm` does both).
export default defineConfig({
  test: {
    include: ['__test__/wasm_*.spec.ts'],
    testTimeout: 120_000,
    hookTimeout: 120_000,
    fileParallelism: false,
  },
})
