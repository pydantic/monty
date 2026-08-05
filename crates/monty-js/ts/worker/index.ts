// The wasm worker path's public surface (`@pydantic/monty/wasm`).
//
// The canonical API is `Monty.create(options)`. Lower-level consumers can call
// `createWorkerPool(module, options)` when they need to supply a compiled
// `WebAssembly.Module` themselves.
//
// `createWorkerPool` picks a browser Web Worker where `Worker` exists, else an
// in-process degrade. The Node export condition has its own `Monty.create`
// wired to `nodeWorkerFactory`; keeping that import in `index.node.ts` prevents
// browser bundles from pulling in `node:worker_threads`.

import { browserWorkerFactory } from './browserFactory.js'
import { type WorkerFactory, WorkerPool, inProcessFactory } from './pool.js'
import { createWorkerPoolFromFactory, workerChannelOptions } from './poolOptions.js'

export interface WasmPoolOptions {
  /** Accepted for parity with the native API; wasm always loads the bundled asset. */
  binaryPath?: string
  /** Workers spawned up front by `create()` (default 1). */
  minProcesses?: number
  /** Worker cap; checkouts beyond it wait (default: host concurrency, or 4). */
  maxProcesses?: number
  /** Seconds to wait for a free worker; omitted waits forever. */
  checkoutTimeout?: number
  /** Hard per-turn deadline in seconds; on expiry the worker is terminated. */
  requestTimeout?: number
  /** Grace in seconds for the `maxDurationSecs` hard backstop; `null` disables it. */
  durationLimitGrace?: number | null
  /** Recycle a worker after serving this many sessions. */
  maxCheckoutsPerWorker?: number
  /** Overrides the worker entry URL used by the browser backend. */
  workerUrl?: string | URL
}

/** Creates a pool over the best backend for this environment. */
export async function createWorkerPool(module: WebAssembly.Module, options: WasmPoolOptions = {}): Promise<WorkerPool> {
  const factory: WorkerFactory =
    'Worker' in globalThis
      ? browserWorkerFactory(module, workerChannelOptions(options), options.workerUrl)
      : inProcessFactory(module)
  const browserConcurrency = typeof navigator === 'undefined' ? undefined : navigator.hardwareConcurrency
  return createWorkerPoolFromFactory(factory, options, browserConcurrency)
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
  MontyFileHandle,
  type MontyFileHandleOptions,
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
