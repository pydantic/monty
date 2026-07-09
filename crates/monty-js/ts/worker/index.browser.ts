// Browser entry for `@pydantic/monty/wasm` (the `browser` export condition):
// the base API plus a `createMonty` that fetches + compiles the bundled wasm.

export * from './index.js'

import { loadModule } from './loadModule.browser.js'
import { makeCreateMonty, type WasmPoolOptions, type WorkerPool } from './index.js'

/** Fetches + compiles the bundled wasm and returns a ready pool. */
export const createMonty = makeCreateMonty(loadModule)

export class Monty {
  /** Loads the bundled wasm module and creates a browser Web Worker-backed pool. */
  static async create(options: WasmPoolOptions = {}): Promise<WorkerPool> {
    return createMonty(options)
  }
}
