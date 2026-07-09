import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { normalizePath, type Plugin } from 'vite'
import { defineConfig } from 'vitest/config'

const pkg = normalizePath(resolve(dirname(fileURLToPath(import.meta.url))))
const nativeIndex = normalizePath(resolve(pkg, 'index.js'))
const browserIndex = normalizePath(resolve(pkg, 'browser.js'))

export default defineConfig({
  optimizeDeps: { exclude: ['@pydantic/monty'] },
  plugins: [montyBrowserIndexPlugin()],
  resolve: {
    alias: [{ find: '@pydantic/monty-wasm32-wasi', replacement: resolve(pkg, 'monty.wasi-browser.js') }],
  },
  server: {
    port: 5179,
    strictPort: true,
    headers: {
      'Cross-Origin-Opener-Policy': 'same-origin',
      'Cross-Origin-Embedder-Policy': 'require-corp',
    },
  },
  test: {
    include: ['browser-test/*.spec.ts'],
    testTimeout: 60_000,
    browser: {
      enabled: true,
      provider: 'playwright',
      headless: true,
      instances: [
        {
          browser: 'chromium',
          launch: { args: ['--enable-features=WebAssemblyUnlimitedSyncCompilation'] },
        },
      ],
    },
  },
})

function montyBrowserIndexPlugin(): Plugin {
  return {
    name: 'monty-browser-index',
    enforce: 'pre',
    resolveId(source, importer) {
      if (importer === undefined || (source !== '../index.js' && normalizePath(source) !== nativeIndex)) {
        return null
      }

      const normalizedImporter = normalizePath(importer)
      if (normalizedImporter.startsWith(`${pkg}/ts/`) || normalizedImporter.startsWith(`${pkg}/dist/`)) {
        return browserIndex
      }

      return null
    },
  }
}
