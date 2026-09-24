import { defineConfig } from 'vitest/config'

// Run the common public contract against real Node worker threads, without napi.
export default defineConfig({
  define: { MONTY_TEST_WASM: true },
  resolve: {
    alias: [
      { find: /^@pydantic\/monty$/, replacement: new URL('./dist/worker/index.node.js', import.meta.url).pathname },
      { find: '@pydantic/monty/node', replacement: new URL('./test-support/node-stubs.ts', import.meta.url).pathname },
    ],
  },
  test: {
    include: ['__test__/*.spec.ts'],
    exclude: ['__test__/node_*.spec.ts'],
    testTimeout: 120_000,
    hookTimeout: 120_000,
    fileParallelism: false,
  },
})
