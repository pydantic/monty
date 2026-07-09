// Loads the bundled wasm module from disk — Node and other WASI/file-system
// hosts. Selected by the `node` condition of the `@pydantic/monty/wasm` export.
//
// The `.wasm` ships next to this module (copied into `dist/worker/` by
// `scripts/copy-wasm.mjs`), so `import.meta.url` resolves it in both the
// published package and local dev.

import { readFile } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'

export async function loadModule(): Promise<WebAssembly.Module> {
  const path = fileURLToPath(new URL('./monty_wasm_runtime.wasm', import.meta.url))
  return WebAssembly.compile(await readFile(path))
}
