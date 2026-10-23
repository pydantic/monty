import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vitest/config'

const nodeBuiltinsStub = fileURLToPath(new URL('./test-support/node-builtins-stub.ts', import.meta.url))

export default defineConfig({
  optimizeDeps: {
    include: ['@bytecodealliance/preview2-shim/instantiation'],
    exclude: ['@pydantic/monty'],
  },
  resolve: {
    alias: {
      '@pydantic/monty/node': fileURLToPath(new URL('./test-support/node-stubs.ts', import.meta.url)),
    },
  },
  plugins: [
    {
      name: 'monty-browser-node-builtins',
      enforce: 'pre',
      // only browser code: the global setup's servers run in Node (ssr) and need the real modules
      resolveId(id, _importer, options) {
        return id.startsWith('node:') && !options.ssr ? nodeBuiltinsStub : null
      },
    },
  ],
  server: {
    port: 5179,
    strictPort: true,
  },
  test: {
    include: ['__test__/*.spec.ts'],
    // `node_*` specs check things about the repo itself (reading source files,
    // shelling out) and cannot run against a filesystem-less browser, where
    // every `node:` import resolves to the throwing stub above
    exclude: ['__test__/node_*.spec.ts'],
    // servers for wasm_websocket.spec.ts, bridged to `monty subprocess` children
    globalSetup: ['__test__/websocket_setup.ts'],
    testTimeout: 60_000,
    hookTimeout: 60_000,
    browser: {
      enabled: true,
      provider: 'playwright',
      headless: true,
      instances: [{ browser: 'chromium' }],
    },
  },
})
