/// <reference lib="dom" />
// Loads the bundled wasm module via fetch + streaming compile — browsers.
// Selected by the `browser` condition of the `@pydantic/monty/wasm` export.
//
// `new URL('./monty_wasm_runtime.wasm', import.meta.url)` is the asset-reference
// pattern Vite/webpack/esbuild recognize to emit and resolve the `.wasm`; in a
// raw-ESM browser it resolves relative to this module. Bundlers/runtimes that
// don't support it can load the module themselves and call `createWorkerPool`.

export async function loadModule(): Promise<WebAssembly.Module> {
  const url = new URL('./monty_wasm_runtime.wasm', import.meta.url)
  return WebAssembly.compileStreaming(fetch(url))
}
