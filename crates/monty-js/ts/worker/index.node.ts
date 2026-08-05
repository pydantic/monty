// Node entry for `@pydantic/monty/wasm` (the `node` export condition): loads
// the bundled wasm asset from disk and runs every instance in a worker thread.

import { availableParallelism } from 'node:os'

import { type WasmPoolOptions, type WorkerPool } from './index.js'
import { loadModule } from './loadModule.node.js'
import { nodeWorkerFactory } from './nodeFactory.js'
import { createWorkerPoolFromFactory, workerChannelOptions } from './poolOptions.js'

export * from './index.js'

/** Creates a hard-preemptible Node worker-thread pool for a compiled module. */
export function createWorkerPool(module: WebAssembly.Module, options: WasmPoolOptions = {}): Promise<WorkerPool> {
  const factory = nodeWorkerFactory(module, workerChannelOptions(options))
  return createWorkerPoolFromFactory(factory, options, availableParallelism())
}

export class Monty {
  /** Loads the bundled wasm and creates a hard-preemptible worker-thread pool. */
  static async create(options: WasmPoolOptions = {}): Promise<WorkerPool> {
    return createWorkerPool(await loadModule(), options)
  }
}
