import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { defineConfig, normalizePath } from 'vite'
import type { Plugin } from 'vite'

const pkg = normalizePath(resolve(dirname(fileURLToPath(import.meta.url)), '..'))
const nativeIndex = normalizePath(resolve(pkg, 'index.js'))
const browserIndex = normalizePath(resolve(pkg, 'browser.js'))

// Excludes the package from esbuild's dep pre-bundling: it uses generated wasm
// loader assets which the pre-bundler may rewrite incorrectly. Rollup (build)
// and the dev server handle those patterns natively.
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
  preview: { port: 5179, strictPort: true },
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
