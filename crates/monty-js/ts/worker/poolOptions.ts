import type { WasmPoolOptions } from './index.js'
import type { WorkerChannelOptions } from './channel.js'
import { type WorkerFactory, WorkerPool, type WorkerPoolOptions } from './pool.js'

/** Creates a pool from an environment-specific worker factory. */
export function createWorkerPoolFromFactory(
  factory: WorkerFactory,
  options: WasmPoolOptions = {},
  defaultMaxProcesses = 4,
): Promise<WorkerPool> {
  return WorkerPool.create(factory, workerPoolOptions(options, defaultMaxProcesses))
}

/** Normalizes public request-timeout options for a worker channel. */
export function workerChannelOptions(options: WasmPoolOptions): WorkerChannelOptions {
  return { requestTimeoutMs: milliseconds(options.requestTimeout, 'requestTimeout') }
}

/** Normalizes the public seconds/process options for the low-level pool. */
function workerPoolOptions(options: WasmPoolOptions, defaultMaxProcesses: number): WorkerPoolOptions {
  return {
    minWorkers: options.minProcesses,
    maxWorkers: options.maxProcesses ?? defaultMaxProcesses,
    checkoutTimeoutMs: milliseconds(options.checkoutTimeout, 'checkoutTimeout'),
    durationLimitGraceMs:
      options.durationLimitGrace === null ? null : milliseconds(options.durationLimitGrace ?? 1, 'durationLimitGrace'),
    maxCheckoutsPerWorker: options.maxCheckoutsPerWorker,
  }
}

/** Converts a validated public seconds option to channel milliseconds. */
function milliseconds(value: number | undefined, name: string): number | undefined {
  if (value === undefined) return undefined
  const milliseconds = value * 1000
  if (!Number.isFinite(value) || value < 0 || !Number.isFinite(milliseconds)) {
    throw new Error(`${name} must be a finite non-negative number`)
  }
  return milliseconds
}
