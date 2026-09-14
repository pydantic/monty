// Browser entry for `@pydantic/monty/wasm` and the root package `browser`
// condition: fetches and compiles the bundled wasm asset for `Monty.create()`.

export * from './index.js'

import { createWorkerPool, type WasmPoolOptions, type WorkerPool } from './index.js'
import { loadModule } from './loadModule.browser.js'

// Exported so an app can load the wasm ahead of time and pass the result to
// `createWorkerPool`.
export { loadModule }

export class Monty {
  /** Loads the bundled wasm module and creates a browser Web Worker-backed pool. */
  static async create(options: WasmPoolOptions = {}): Promise<WorkerPool> {
    return createWorkerPool(await loadModule(), options)
  }
}
