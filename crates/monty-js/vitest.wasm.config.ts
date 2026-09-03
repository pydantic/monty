import { defineConfig } from 'vitest/config'

// The wasm worker driven from Node, with no browser: needs the generated files
// under `dist/worker/component`, so run `npm run build:wasm` first (`make
// test-wasm` does both).
export default defineConfig({
  test: {
    include: ['__test__/wasm_*.spec.ts'],
    testTimeout: 120_000,
    hookTimeout: 120_000,
    fileParallelism: false,
  },
})
