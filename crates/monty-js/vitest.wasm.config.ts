import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vitest/config'

// Run the common public contract against real Node worker threads, without napi.
export default defineConfig({
  define: { MONTY_TEST_WASM: true },
  resolve: {
    alias: [
      {
        find: /^@pydantic\/monty$/,
        replacement: fileURLToPath(new URL('./dist/worker/index.node.js', import.meta.url)),
      },
      {
        find: '@pydantic/monty/node',
        replacement: fileURLToPath(new URL('./test-support/node-stubs.ts', import.meta.url)),
      },
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
