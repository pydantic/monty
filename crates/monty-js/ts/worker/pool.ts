// A TypeScript reimplementation of `monty-pool` for the wasm worker path.
//
// One worker runs one session at a time. A clean close resets and returns it;
// a crash, protocol failure, or checkout quota retires it. Browser and Node
// worker backends provide hard turn preemption, while `inProcessFactory`
// remains an explicit degrade for runtimes without workers.

import { MontySession } from '../session.js'
import { deadlineTimer, type DeadlineTimer } from './deadline.js'
import { WasmHost, type Dispatcher, inProcessDispatcher } from './host.js'
import { WorkerTransport, type WorkerSessionConfig } from './transport.js'

/** `MontySession`'s constructor argument (the structural `NativeSession`). */
type SessionNative = ConstructorParameters<typeof MontySession>[0]

/** A spawned worker: a dispatch channel plus a hard-kill primitive. */
export interface PooledWorker {
  /** Sends one framed request and resolves to the framed reply. */
  readonly dispatch: Dispatcher
  /** Force-terminates the worker (`Worker.terminate()`; a no-op in-process). */
  terminate(): void | Promise<void>
  /** Becomes false once the worker has died or been terminated. */
  readonly alive: boolean
  /** Runtime-specific worker identity, when one exists. */
  readonly workerId?: number
  /** OS process identity, available only to process-backed factories. */
  readonly workerPid?: number
}

/** Spawns a fresh worker for the pool. */
export type WorkerFactory = () => Promise<PooledWorker>

/**
 * Creates in-process wasm workers for runtimes without a Worker primitive and
 * low-level tests. A runaway turn cannot be preempted in this backend.
 */
export function inProcessFactory(module: WebAssembly.Module): WorkerFactory {
  return async () => {
    const host = await WasmHost.create(module)
    const dispatch = inProcessDispatcher(host)
    let alive = true
    return {
      dispatch,
      terminate() {
        alive = false
      },
      get alive() {
        return alive
      },
    }
  }
}

export interface WorkerPoolOptions {
  /** Workers kept warm even when idle. Default 1. */
  minWorkers?: number
  /** Hard ceiling on live workers; further checkouts wait. Default 4. */
  maxWorkers?: number
  /** Milliseconds to wait for pool capacity. Omitted means forever. */
  checkoutTimeoutMs?: number
  /** Grace for the `maxDurationSecs` hard backstop. `null` disables it. */
  durationLimitGraceMs?: number | null
  /** Recycle (terminate + replace) a worker after this many checkouts. */
  maxCheckoutsPerWorker?: number
}

/** One pooled worker with its checkout bookkeeping. */
interface WorkerSlot {
  readonly worker: PooledWorker
  readonly id: number
  checkouts: number
}

interface Waiter {
  resolve(slot: WorkerSlot): void
  reject(err: Error): void
  timer: DeadlineTimer | null
}

/**
 * An elastic pool of wasm workers. `checkout` dedicates one worker to a REPL
 * session and the session returns or retires it on close/death.
 */
export class WorkerPool {
  private readonly idle: WorkerSlot[] = []
  private readonly waiters: Waiter[] = []
  /** Live workers: idle + checked out + mid-spawn. */
  private total = 0
  private nextWorkerId = 1
  private closed = false
  private closePromise: Promise<void> | null = null

  private constructor(
    private readonly factory: WorkerFactory,
    private readonly maxWorkers: number,
    private readonly checkoutTimeoutMs: number | undefined,
    private readonly durationLimitGraceMs: number | undefined,
    private readonly maxCheckouts: number | undefined,
  ) {}

  /** Creates the pool and prewarms `minWorkers` idle workers. */
  static async create(factory: WorkerFactory, options: WorkerPoolOptions = {}): Promise<WorkerPool> {
    const max = integerOption(options.maxWorkers ?? 4, 'maxWorkers')
    const min = integerOption(options.minWorkers ?? 1, 'minWorkers')
    if (max < 1) throw new Error('maxProcesses must be at least 1')
    if (min > max) throw new Error('minProcesses cannot exceed maxProcesses')
    const checkoutTimeoutMs = timeoutOption(options.checkoutTimeoutMs, 'checkoutTimeout')
    const durationLimitGraceMs =
      options.durationLimitGraceMs === null
        ? undefined
        : timeoutOption(options.durationLimitGraceMs ?? 1000, 'durationLimitGrace')
    const maxCheckouts =
      options.maxCheckoutsPerWorker === undefined
        ? undefined
        : integerOption(options.maxCheckoutsPerWorker, 'maxCheckoutsPerWorker')
    const pool = new WorkerPool(factory, max, checkoutTimeoutMs, durationLimitGraceMs, maxCheckouts)
    const results = await Promise.allSettled(Array.from({ length: min }, () => pool.spawn()))
    const warm: WorkerSlot[] = []
    let failed = false
    let failure: unknown
    for (const result of results) {
      if (result.status === 'fulfilled') warm.push(result.value)
      else if (!failed) {
        failed = true
        failure = result.reason
      }
    }
    if (failed) {
      await Promise.allSettled(warm.map((slot) => slot.worker.terminate()))
      throw failure
    }
    pool.idle.push(...warm)
    return pool
  }

  /** Borrows a worker and returns a session bound to it. */
  async checkout(config: WorkerSessionConfig = {}): Promise<MontySession> {
    if (this.closed) throw new Error('the pool is closed — create a new Monty pool')
    const slot = await this.acquire()
    let transport: WorkerTransport
    try {
      transport = await WorkerTransport.create(slot.worker.dispatch, config, {
        durationLimitGraceMs: this.durationLimitGraceMs,
        workerId: slot.id,
        workerPid: slot.worker.workerPid,
      })
    } catch (err) {
      this.discard(slot)
      throw err
    }
    transport.onFinish = (reusable) => this.release(slot, reusable)
    return new MontySession(transport as unknown as SessionNative)
  }

  /** Terminates idle workers and rejects checkouts still waiting for capacity. */
  close(): Promise<void> {
    this.closePromise ??= this.closeWorkers()
    return this.closePromise
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close()
  }

  /** Live worker count, for diagnostics and low-level pool tests. */
  get size(): number {
    return this.total
  }

  /** Performs pool shutdown once; every `close` caller awaits this operation. */
  private async closeWorkers(): Promise<void> {
    this.closed = true
    for (const waiter of this.waiters.splice(0)) {
      waiter.timer?.cancel()
      waiter.reject(new Error('the pool is closed — create a new Monty pool'))
    }
    const idle = this.idle.splice(0)
    this.total -= idle.length
    await Promise.all(idle.map((slot) => slot.worker.terminate()))
  }

  /** Reuses, spawns, or waits for one live worker slot. */
  private async acquire(): Promise<WorkerSlot> {
    while (this.idle.length > 0) {
      const slot = this.idle.pop()!
      if (slot.worker.alive) return slot
      void slot.worker.terminate()
      this.total--
    }
    if (this.total < this.maxWorkers) return this.spawn()
    return new Promise<WorkerSlot>((resolve, reject) => {
      const waiter: Waiter = { resolve, reject, timer: null }
      if (this.checkoutTimeoutMs !== undefined) {
        waiter.timer = deadlineTimer(this.checkoutTimeoutMs, () => {
          const index = this.waiters.indexOf(waiter)
          if (index !== -1) {
            this.waiters.splice(index, 1)
            reject(new Error('no monty worker became available within the checkout timeout'))
          }
        })
      }
      this.waiters.push(waiter)
    })
  }

  /** Returns a worker after a session ends; reuse, recycle, or discard. */
  private release(slot: WorkerSlot, reusable: boolean): void {
    slot.checkouts++
    const recycle = this.maxCheckouts !== undefined && slot.checkouts >= this.maxCheckouts
    if (this.closed || !reusable || !slot.worker.alive || recycle) {
      this.discard(slot)
    } else {
      const waiter = this.takeWaiter()
      if (waiter) waiter.resolve(slot)
      else this.idle.push(slot)
    }
  }

  /** Terminates a worker, frees its capacity, and serves pending checkouts. */
  private discard(slot: WorkerSlot): void {
    void slot.worker.terminate()
    this.total--
    this.pump()
  }

  /** Spawns replacements for checkouts waiting on newly freed capacity. */
  private pump(): void {
    while (!this.closed && this.waiters.length > 0 && this.total < this.maxWorkers) {
      const waiter = this.takeWaiter()!
      this.spawn().then(
        (slot) => waiter.resolve(slot),
        (err) => {
          waiter.reject(asError(err))
          this.pump()
        },
      )
    }
  }

  /** Takes the oldest capacity waiter and disarms its checkout deadline. */
  private takeWaiter(): Waiter | undefined {
    const waiter = this.waiters.shift()
    waiter?.timer?.cancel()
    return waiter
  }

  /** Creates one worker, counting it against `total` until failure/disposal. */
  private async spawn(): Promise<WorkerSlot> {
    this.total++
    try {
      const worker = await this.factory()
      return { worker, id: worker.workerId ?? this.nextWorkerId++, checkouts: 0 }
    } catch (err) {
      this.total--
      throw err
    }
  }
}

/** Validates a non-negative process/count option. */
function integerOption(value: number, name: string): number {
  if (!Number.isSafeInteger(value) || value < 0) throw new Error(`${name} must be a non-negative safe integer`)
  return value
}

/** Validates an optional non-negative millisecond timeout. */
function timeoutOption(value: number | undefined, name: string): number | undefined {
  if (value === undefined) return undefined
  if (!Number.isFinite(value) || value < 0) throw new Error(`${name} must be a finite non-negative number`)
  return value
}

/** Normalizes a thrown JavaScript value for promise rejection. */
function asError(err: unknown): Error {
  return err instanceof Error ? err : new Error(String(err))
}
