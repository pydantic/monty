import type { Monty as NativeMonty, MontyOptions } from '../pool.js'
import type { WorkerChannelOptions } from './channel.js'
import { timeoutOption } from './deadline.js'
import { WorkerPool, type WorkerFactory } from './pool.js'

/** The public pool methods, without either backend's implementation details. */
export type Monty = Pick<NativeMonty, keyof NativeMonty>

/** Shared pool configuration plus browser worker asset resolution. */
export interface WasmPoolOptions extends MontyOptions {
  /** Ignored by WASM, which loads the bundled component instead. */
  binaryPath?: string
  /** Defaults to host concurrency, or 4 when unavailable. */
  maxProcesses?: number
  /** Overrides the browser worker entry URL. */
  workerUrl?: string | URL
}

/** Applies the same public pool options to both worker backends. */
export function createWorkerPoolFromFactory(
  factory: WorkerFactory,
  options: WasmPoolOptions,
  concurrency: number,
): Promise<Monty> {
  return WorkerPool.create(factory, {
    minWorkers: options.minProcesses,
    maxWorkers: options.maxProcesses ?? concurrency,
    maxCheckoutsPerWorker: options.maxCheckoutsPerWorker,
    checkoutTimeoutMs: seconds(options.checkoutTimeout, 'checkoutTimeout'),
    feedDurationLimitGraceMs:
      options.feedDurationLimitGrace === null
        ? null
        : seconds(options.feedDurationLimitGrace ?? 1, 'feedDurationLimitGrace'),
    turnDurationLimitGraceMs:
      options.turnDurationLimitGrace === null
        ? null
        : seconds(options.turnDurationLimitGrace ?? 1, 'turnDurationLimitGrace'),
  })
}

/** Validates the channel's independent hard per-request deadline. */
export function workerChannelOptions(options: WasmPoolOptions): WorkerChannelOptions {
  return { requestTimeoutMs: seconds(options.requestTimeout, 'requestTimeout') }
}

/** Converts public seconds to validated host timer milliseconds. */
function seconds(value: number | undefined, name: string): number | undefined {
  return timeoutOption(value === undefined ? undefined : value * 1000, name)
}
