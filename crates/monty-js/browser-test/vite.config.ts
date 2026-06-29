import { defineConfig } from 'vite'

// Excludes the package from esbuild's dep pre-bundling: it uses
// `new URL(..., import.meta.url)` for the wasm asset and the worker entry,
// which the pre-bundler mangles. Rollup (build) and the dev server handle those
// patterns natively. No COOP/COEP headers are set — verifying the worker path
// needs no cross-origin isolation.
export default defineConfig({
  optimizeDeps: { exclude: ['@pydantic/monty'] },
  server: { port: 5179, strictPort: true },
  preview: { port: 5179, strictPort: true },
})
