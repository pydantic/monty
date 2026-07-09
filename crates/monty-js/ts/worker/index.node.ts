// Node entry for `@pydantic/monty/wasm` (the `node` export condition): loads
// the bundled wasm asset from disk for `Monty.create()`.

export * from './index.js'

import { createWorkerPool, type WasmPoolOptions, type WorkerPool } from './index.js'
import { loadModule } from './loadModule.node.js'

export class Monty {
  /** Loads the bundled wasm from disk and creates a worker-backed pool. */
  static async create(options: WasmPoolOptions = {}): Promise<WorkerPool> {
    return createWorkerPool(await loadModule(), options)
  }
}
