import { defineConfig } from 'vitest/config'

export default defineConfig({
  test: {
    include: ['__test__/*.spec.ts'],
    // the wasm specs need a built `monty_wasm_runtime.wasm`, which this suite
    // does not build — `vitest.wasm.config.ts` runs those
    exclude: ['__test__/wasm_*.spec.ts'],
    testTimeout: 120_000,
    hookTimeout: 120_000,
    fileParallelism: false,
  },
})
