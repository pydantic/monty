// The wasm worker path's public surface (`@pydantic/monty/wasm`).
//
// The canonical API is `Monty.create(options)`. Lower-level consumers can call
// `createWorkerPool(module, options)` when they need to supply a compiled
// `WebAssembly.Module` themselves.
//
// `createWorkerPool` picks the backend: a browser Web Worker where
// `Worker` exists (off-thread + a hard-kill watchdog), else in-process wasm as
// a degrade (same API, no crash isolation or preemption). Node users wanting
// real threads import `nodeWorkerFactory` from `./nodeFactory.js` directly
// (separate so browser bundles never pull in `node:worker_threads`).

import { browserWorkerFactory } from './browserFactory.js'
import { type WorkerFactory, WorkerPool, inProcessFactory } from './pool.js'

export interface WasmPoolOptions {
  /** Accepted for parity with the native API; wasm always loads the bundled asset. */
  binaryPath?: string
  /** Workers spawned up front by `create()` (default 1). */
  minProcesses?: number
  /** Worker cap; checkouts beyond it wait (default 4). */
  maxProcesses?: number
  /** Accepted for parity with the native API; wasm currently waits forever. */
  checkoutTimeout?: number
  /** Hard per-turn deadline in seconds; on expiry the worker is terminated. */
  requestTimeout?: number
  /** Accepted for parity with the native API; wasm uses in-sandbox limits only. */
  durationLimitGrace?: number | null
  /** Recycle a worker after serving this many sessions. */
  maxCheckoutsPerWorker?: number
  /** Overrides the worker entry URL used by the browser backend. */
  workerUrl?: string | URL
}

/** Creates a pool over the best backend for this environment. */
export async function createWorkerPool(module: WebAssembly.Module, options: WasmPoolOptions = {}): Promise<WorkerPool> {
  const requestTimeoutMs = options.requestTimeout === undefined ? undefined : options.requestTimeout * 1000
  const factory: WorkerFactory =
    'Worker' in globalThis
      ? browserWorkerFactory(module, { requestTimeoutMs }, options.workerUrl)
      : inProcessFactory(module)
  return WorkerPool.create(factory, {
    minWorkers: options.minProcesses,
    maxWorkers: options.maxProcesses,
    maxCheckoutsPerWorker: options.maxCheckoutsPerWorker,
  })
}

/** Loads the bundled wasm module and creates a browser/worker-backed pool. */
export class Monty {
  static async create(_options: WasmPoolOptions = {}): Promise<WorkerPool> {
    throw new Error(
      'Monty.create could not auto-load the monty wasm module in this environment; ' +
        'compile it yourself and call createWorkerPool(module) instead',
    )
  }
}

export { WorkerPool, inProcessFactory } from './pool.js'
export {
  FunctionSnapshot,
  FutureSnapshot,
  MontyComplete,
  MontySession,
  NameLookupSnapshot,
  NOT_HANDLED,
} from '../session.js'
export type {
  ExternalFunction,
  FeedOptions,
  FeedStartOptions,
  FutureResolution,
  LoadSnapshotOptions,
  OsCallback,
  PrintCallback,
  PrintTargetInput,
  Snapshot,
} from '../session.js'
export { CollectString, CollectStreams, DEFAULT_MAX_PRINT_COLLECT_BYTES, type CollectedStreamEntry } from '../print.js'
export {
  MontyCrashedError,
  MontyError,
  MontyRuntimeError,
  MontySyntaxError,
  MontyTypingError,
  ProtocolError,
  type ExceptionInfo,
  type Frame,
} from '../errors.js'
export {
  type MontyDate,
  type MontyDateTime,
  type MontyException,
  type MontyFileHandle,
  type MontyTimeDelta,
  type MontyTimeZone,
} from '../types.js'
export type { PooledWorker, WorkerFactory, WorkerPoolOptions } from './pool.js'
export { WorkerTransport } from './transport.js'
export type { ResourceLimits, WorkerSessionConfig } from './transport.js'
export { WasmHost, inProcessDispatcher } from './host.js'
export type { Dispatcher } from './host.js'
export { WorkerChannel } from './channel.js'
export type { WorkerChannelOptions, WorkerLike } from './channel.js'
export { browserWorkerFactory } from './browserFactory.js'
