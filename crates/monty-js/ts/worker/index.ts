// The wasm worker path's public surface (`@pydantic/monty/wasm`): one API
// across environments.
//
// Two layers:
//   - `createMonty(options)` — turnkey: loads the bundled wasm module for this
//     environment and returns a ready `WorkerPool`. The `node`/`browser`
//     conditional exports swap in the right loader; this default entry throws a
//     clear error in environments matching neither, directing callers to
//     `createWorkerPool`.
//   - `createWorkerPool(module, options)` — the explicit escape hatch: you
//     supply the compiled `WebAssembly.Module`, for bundlers/runtimes the
//     turnkey loader does not cover.
//
// Either way, `createWorkerPool` picks the backend: a browser Web Worker where
// `Worker` exists (off-thread + a hard-kill watchdog), else in-process wasm as
// a degrade (same API, no crash isolation or preemption). Node users wanting
// real threads import `nodeWorkerFactory` from `./nodeFactory.js` directly
// (separate so browser bundles never pull in `node:worker_threads`).

import { browserWorkerFactory } from './browserFactory.js'
import { type WorkerFactory, WorkerPool, type WorkerPoolOptions, inProcessFactory } from './pool.js'

export interface WasmPoolOptions extends WorkerPoolOptions {
  /** Hard per-turn deadline; on expiry the worker is terminated (Worker backend). */
  requestTimeoutMs?: number
  /** Overrides the worker entry URL used by the browser backend. */
  workerUrl?: string | URL
}

/** Loads the bundled wasm module for the current environment. */
export type ModuleLoader = () => Promise<WebAssembly.Module>

/** Creates a pool over the best backend for this environment. */
export async function createWorkerPool(module: WebAssembly.Module, options: WasmPoolOptions = {}): Promise<WorkerPool> {
  const factory: WorkerFactory =
    'Worker' in globalThis
      ? browserWorkerFactory(module, { requestTimeoutMs: options.requestTimeoutMs }, options.workerUrl)
      : inProcessFactory(module)
  return WorkerPool.create(factory, options)
}

/** Builds a turnkey `createMonty` over an environment-specific module loader. */
export function makeCreateMonty(loadModule: ModuleLoader) {
  return async (options: WasmPoolOptions = {}): Promise<WorkerPool> => createWorkerPool(await loadModule(), options)
}

/**
 * Loads the bundled wasm and returns a ready pool. This default implementation
 * throws — the `node` / `browser` conditional exports replace it with a real
 * loader. In an unrecognized environment, load the module yourself and call
 * `createWorkerPool`.
 */
export const createMonty = makeCreateMonty(() =>
  Promise.reject(
    new Error(
      'createMonty could not auto-load the monty wasm module in this environment; ' +
        'compile it yourself and call createWorkerPool(module) instead',
    ),
  ),
)

export { WorkerPool, inProcessFactory } from './pool.js'
export type { PooledWorker, WorkerFactory, WorkerPoolOptions } from './pool.js'
export { WorkerTransport } from './transport.js'
export type { ResourceLimits, WorkerSessionConfig } from './transport.js'
export { WasmHost, inProcessDispatcher } from './host.js'
export type { Dispatcher } from './host.js'
export { WorkerChannel } from './channel.js'
export type { WorkerChannelOptions, WorkerLike } from './channel.js'
export { browserWorkerFactory } from './browserFactory.js'
