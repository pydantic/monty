import { defineConfig } from 'vite'

export default defineConfig({
  // Asset URLs relative to the page rather than the site root, so `dist/` can
  // be served under any path prefix.
  base: './',
  // `@pydantic/monty` is a file: link to this repo's crate. Prebundling it
  // would flatten away the `new URL(..., import.meta.url)` references that
  // Vite needs to see in order to emit the component's wasm assets and the
  // Web Worker entry as their own chunks.
  optimizeDeps: { exclude: ['@pydantic/monty'] },
  // The worker entry imports the component's modules, so it has to be built as
  // an ES module; Vite's default of `iife` cannot code-split.
  worker: { format: 'es' },
  // `main.ts` starts the pool with a top-level await, which Vite's default
  // browser target predates.
  build: { target: 'es2022' },
  // Those assets then live outside this directory, which the dev server does
  // not serve by default. An app depending on the published package needs
  // neither of these settings.
  server: { fs: { allow: ['.', '../../crates/monty-js'] } },
})
