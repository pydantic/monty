import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

import { defineConfig } from 'vite'

const pkg = resolve(dirname(fileURLToPath(import.meta.url)), '..')

// Excludes the package from esbuild's dep pre-bundling: it uses generated wasm
// loader assets which the pre-bundler may rewrite incorrectly. Rollup (build)
// and the dev server handle those patterns natively.
export default defineConfig({
  optimizeDeps: { exclude: ['@pydantic/monty'] },
  resolve: {
    alias: [
      { find: /^\.\.\/index\.js$/, replacement: resolve(pkg, 'browser.js') },
      { find: '@pydantic/monty-wasm32-wasi', replacement: resolve(pkg, 'monty.wasi-browser.js') },
      { find: resolve(pkg, 'index.js'), replacement: resolve(pkg, 'browser.js') },
    ],
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
