import { defineConfig } from 'vitest/config'

const nodeBuiltinsStub = new URL('./test-support/node-builtins-stub.ts', import.meta.url).pathname

export default defineConfig({
  optimizeDeps: { exclude: ['@pydantic/monty'] },
  resolve: {
    alias: {
      '@pydantic/monty/node': new URL('./test-support/node-stubs.ts', import.meta.url).pathname,
    },
  },
  plugins: [
    {
      name: 'monty-browser-node-builtins',
      enforce: 'pre',
      resolveId(id) {
        return id.startsWith('node:') ? nodeBuiltinsStub : null
      },
    },
  ],
  server: {
    port: 5179,
    strictPort: true,
  },
  test: {
    include: ['__test__/*.spec.ts'],
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
