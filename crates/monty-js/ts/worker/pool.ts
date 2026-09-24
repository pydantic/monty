// An elastic pool of real workers sharing the native pool's checkout and shutdown contract.

import { encodeOsPolicy } from '../options.js'
import { MontySession } from '../session.js'
import { deadlineTimer, timeoutOption, type DeadlineTimer } from './deadline.js'
import type { Dispatcher } from './host.js'
import { WorkerTransport, type WorkerSessionConfig, type DurationGraces } from './transport.js'

/** The transport seam consumed by the shared session drive loop. */
type SessionNative = ConstructorParameters<typeof MontySession>[0]

/** A worker with a dispatch channel and a hard-kill primitive. */
export interface PooledWorker {
  readonly dispatch: Dispatcher
  terminate(): void | Promise<void>
  readonly alive: boolean
}

/** Spawns a fresh worker for the pool. */
export type WorkerFactory = () => Promise<PooledWorker>

/** Pool capacity, checkout deadline and duration-backstop configuration. */
export interface WorkerPoolOptions extends DurationGraces {
  minWorkers?: number
  maxWorkers?: number
  checkoutTimeoutMs?: number
  maxCheckoutsPerWorker?: number
}

/** One worker and its checkout bookkeeping. */
interface WorkerSlot {
  readonly worker: PooledWorker
  readonly id: number
  checkouts: number
}

/** A pending checkout, removed from the queue when timed out. */
interface Waiter {
  resolve(slot: WorkerSlot): void
  reject(err: Error): void
  timer: DeadlineTimer | null
}

/** A worker-backed pool; checked-out sessions remain usable after pool shutdown. */
export class WorkerPool {
  private readonly idle: WorkerSlot[] = []
  private readonly waiters: Waiter[] = []
  private readonly retiring = new Set<Promise<void>>()
  private total = 0
  private nextId = 1
  private closed = false
  private closing: Promise<void> | undefined

  private constructor(
    private readonly factory: WorkerFactory,
    private readonly maxWorkers: number,
    private readonly options: WorkerPoolOptions,
  ) {}

  /** Creates the pool and prewarms its minimum number of workers. */
  static async create(factory: WorkerFactory, options: WorkerPoolOptions = {}): Promise<WorkerPool> {
    const max = integerOption(options.maxWorkers ?? 4, 'maxProcesses', 1)
    const min = integerOption(options.minWorkers ?? 1, 'minProcesses', 0)
    if (min > max) throw new TypeError('minProcesses cannot exceed maxProcesses')
    if (options.maxCheckoutsPerWorker !== undefined) {
      integerOption(options.maxCheckoutsPerWorker, 'maxCheckoutsPerWorker', 0)
    }
    timeoutOption(options.checkoutTimeoutMs, 'checkoutTimeout')
    timeoutOption(options.feedDurationLimitGraceMs ?? undefined, 'feedDurationLimitGrace')
    timeoutOption(options.turnDurationLimitGraceMs ?? undefined, 'turnDurationLimitGrace')
    const pool = new WorkerPool(factory, max, options)
    // Wait for every spawn before handling failure, so no late worker escapes cleanup.
    const warm = await Promise.allSettled(Array.from({ length: min }, () => pool.spawn()))
    for (const result of warm) {
      if (result.status === 'fulfilled') pool.idle.push(result.value)
    }
    const failure = warm.find((result) => result.status === 'rejected')
    if (failure) {
      await pool.close()
      throw failure.reason
    }
    return pool
  }

  /** Borrows a worker, configuring a new isolated session before returning it. */
  async checkout(config: WorkerSessionConfig = {}): Promise<MontySession> {
    if (this.closed) throw closedError()
    encodeOsPolicy(config.osPolicy ?? {})
    const slot = await this.acquire()
    if (this.closed) {
      await this.discard(slot)
      throw closedError()
    }
    let transport: WorkerTransport
    try {
      transport = await WorkerTransport.create(slot.worker.dispatch, config, this.options, slot.id)
    } catch (error) {
      await this.discard(slot)
      throw error
    }
    if (this.closed) {
      await this.discard(slot)
      throw closedError()
    }
    transport.onFinish = (reusable) => this.release(slot, reusable)
    return new MontySession(transport as unknown as SessionNative)
  }

  /** Rejects new/waiting checkouts and reaps idle workers; active sessions retain their workers. */
  close(): Promise<void> {
    this.closing ??= this.closeIdle()
    return this.closing
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close()
  }

  /** Stops admitting sessions before waiting for worker termination. */
  private async closeIdle(): Promise<void> {
    this.closed = true
    for (const waiter of this.waiters.splice(0)) {
      waiter.timer?.cancel()
      waiter.reject(closedError())
    }
    await Promise.all([...this.idle.splice(0).map((slot) => this.discard(slot)), ...this.retiring])
  }

  /** Reuses a live slot or waits for capacity, removing timed-out waiters. */
  private async acquire(): Promise<WorkerSlot> {
    while (this.idle.length > 0) {
      const slot = this.idle.pop()!
      if (slot.worker.alive) return slot
      await this.discard(slot)
    }
    if (this.closed) throw closedError()
    if (this.total < this.maxWorkers) return this.spawn()
    return new Promise<WorkerSlot>((resolve, reject) => {
      const waiter: Waiter = { resolve, reject, timer: null }
      if (this.options.checkoutTimeoutMs !== undefined) {
        waiter.timer = deadlineTimer(this.options.checkoutTimeoutMs, () => {
          const index = this.waiters.indexOf(waiter)
          if (index >= 0) this.waiters.splice(index, 1)
          reject(new Error('no monty worker became available within the checkout timeout'))
        })
      }
      this.waiters.push(waiter)
    })
  }

  /** Returns clean sessions to waiters and retires exhausted or dead workers. */
  private async release(slot: WorkerSlot, reusable: boolean): Promise<void> {
    slot.checkouts++
    const recycle =
      this.options.maxCheckoutsPerWorker !== undefined && slot.checkouts >= this.options.maxCheckoutsPerWorker
    if (this.closed || !reusable || !slot.worker.alive || recycle) {
      await this.discard(slot)
    } else {
      const waiter = this.waiters.shift()
      if (waiter) {
        waiter.timer?.cancel()
        waiter.resolve(slot)
      } else {
        this.idle.push(slot)
      }
    }
  }

  /** Frees capacity only after termination, so replacements cannot exceed the worker cap. */
  private discard(slot: WorkerSlot): Promise<void> {
    const retiring = Promise.resolve(slot.worker.terminate()).then(() => {
      this.total--
      this.retiring.delete(retiring)
      this.pump()
    })
    this.retiring.add(retiring)
    return retiring
  }

  /** Serves queued checkouts when a discarded worker frees capacity. */
  private pump(): void {
    while (!this.closed && this.waiters.length > 0 && this.total < this.maxWorkers) {
      const waiter = this.waiters.shift()!
      waiter.timer?.cancel()
      this.spawn().then(waiter.resolve, (error) => {
        waiter.reject(error instanceof Error ? error : new Error(String(error)))
        this.pump()
      })
    }
  }

  /** Reserves capacity while the worker initializes, rolling back on failure. */
  private async spawn(): Promise<WorkerSlot> {
    this.total++
    try {
      return { worker: await this.factory(), id: this.nextId++, checkouts: 0 }
    } catch (error) {
      this.total--
      throw error
    }
  }
}

/** Rejects invalid capacities instead of silently changing pool semantics. */
function integerOption(value: number, name: string, minimum: number): number {
  if (!Number.isSafeInteger(value) || value < minimum) {
    throw new TypeError(`${name} must be an integer >= ${minimum}`)
  }
  return value
}

/** The shared native/WASM closed-pool diagnostic. */
function closedError(): Error {
  return new Error('the pool is closed — create a new Monty pool')
}
