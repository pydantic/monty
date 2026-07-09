// A TypeScript reimplementation of `monty-pool` for the wasm worker path.
//
// `monty-pool` (the Rust pool the napi binding uses) is `std::process`/
// `std::thread`-bound, so the browser gets this: the same elasticity, checkout,
// recycle and crash-replacement *logic* over a pluggable worker backend —
// in-process wasm instances for environments without `Worker`, or Web Workers
// (whose `terminate()` is the hard-kill watchdog primitive) in the browser.
//
// One worker runs one session at a time. Checkout builds a `WorkerTransport`
// (sending `ReplCreate`) over the worker's dispatch channel and wraps it in a
// `MontySession`; closing the session `Reset`s the worker and returns it to the
// idle set, unless it should be recycled (served its checkout quota) or died
// (returned to the factory for disposal). State never leaks between sessions:
// `Reset` clears the REPL before reuse.
//
// Not yet ported: the per-turn watchdog timeout. Hard preemption needs a real
// `Worker.terminate()`; with the in-process backend a runaway turn cannot be
// interrupted, exactly as the plan notes.

import { MontySession } from '../session.js'
import { WasmHost, type Dispatcher, inProcessDispatcher } from './host.js'
import { WorkerTransport, type WorkerSessionConfig } from './transport.js'

/** `MontySession`'s constructor argument (the structural `NativeSession`). */
type SessionNative = ConstructorParameters<typeof MontySession>[0]

/** A spawned worker: a dispatch channel plus a hard-kill primitive. */
export interface PooledWorker {
  /** Sends one framed request and resolves to the framed reply. */
  readonly dispatch: Dispatcher
  /** Force-terminates the worker (`Worker.terminate()`; a no-op in-process). */
  terminate(): void
  /** Becomes false once the worker has died or been terminated. */
  readonly alive: boolean
}

/** Spawns a fresh worker for the pool. */
export type WorkerFactory = () => Promise<PooledWorker>

/**
 * A factory of in-process wasm workers (one `WasmHost` per worker), for
 * environments without `Worker` and for tests. `terminate` only flips `alive`
 * (an in-process instance cannot be force-killed); session isolation still
 * holds via `Reset`, but a runaway turn cannot be preempted.
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
  /** Recycle (terminate + replace) a worker after this many checkouts. */
  maxCheckoutsPerWorker?: number
}

/** One pooled worker with its checkout bookkeeping. */
interface WorkerSlot {
  readonly worker: PooledWorker
  checkouts: number
}

interface Waiter {
  resolve(slot: WorkerSlot): void
  reject(err: Error): void
}

/**
 * An elastic pool of wasm workers. Mirrors `pydantic_monty`'s `Monty` /
 * `monty-pool`: `checkout` borrows a worker for one session, the session
 * returns it on close.
 */
export class WorkerPool {
  private readonly idle: WorkerSlot[] = []
  private readonly waiters: Waiter[] = []
  /** Live workers: idle + checked out + mid-spawn. */
  private total = 0
  private closed = false

  private constructor(
    private readonly factory: WorkerFactory,
    private readonly maxWorkers: number,
    private readonly maxCheckouts: number | undefined,
  ) {}

  /** Creates the pool and prewarms `minWorkers` idle workers. */
  static async create(factory: WorkerFactory, options: WorkerPoolOptions = {}): Promise<WorkerPool> {
    const max = Math.max(1, options.maxWorkers ?? 4)
    const min = Math.min(Math.max(0, options.minWorkers ?? 1), max)
    const pool = new WorkerPool(factory, max, options.maxCheckoutsPerWorker)
    const warm = await Promise.all(Array.from({ length: min }, () => pool.spawn()))
    pool.idle.push(...warm)
    return pool
  }

  /** Borrows a worker and returns a session bound to it. */
  async checkout(config: WorkerSessionConfig = {}): Promise<MontySession> {
    if (this.closed) throw new Error('pool is closed')
    const slot = await this.acquire()
    let transport: WorkerTransport
    try {
      transport = await WorkerTransport.create(slot.worker.dispatch, config)
    } catch (err) {
      this.discard(slot)
      throw err
    }
    transport.onFinish = (reusable) => this.release(slot, reusable)
    return new MontySession(transport as unknown as SessionNative)
  }

  /** Terminates every worker and rejects anyone still waiting. */
  async close(): Promise<void> {
    this.closed = true
    for (const waiter of this.waiters.splice(0)) waiter.reject(new Error('pool is closed'))
    for (const slot of this.idle.splice(0)) {
      slot.worker.terminate()
      this.total--
    }
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close()
  }

  /** Live worker count, for tests/diagnostics. */
  get size(): number {
    return this.total
  }

  // === internals ===

  private async acquire(): Promise<WorkerSlot> {
    while (this.idle.length > 0) {
      const slot = this.idle.pop()!
      if (slot.worker.alive) return slot
      this.total-- // a worker that died while idle; reap and try the next
    }
    if (this.total < this.maxWorkers) return this.spawn()
    return new Promise<WorkerSlot>((resolve, reject) => this.waiters.push({ resolve, reject }))
  }

  /** Returns a worker after a session ends; reuse, recycle, or discard. */
  private release(slot: WorkerSlot, reusable: boolean): void {
    slot.checkouts++
    const recycle = this.maxCheckouts !== undefined && slot.checkouts >= this.maxCheckouts
    if (this.closed || !reusable || !slot.worker.alive || recycle) {
      this.discard(slot)
      return
    }
    const waiter = this.waiters.shift()
    if (waiter) waiter.resolve(slot)
    else this.idle.push(slot)
  }

  /** Terminates a worker, frees its capacity, and serves any waiters. */
  private discard(slot: WorkerSlot): void {
    slot.worker.terminate()
    this.total--
    this.pump()
  }

  /** Spawns replacements for any checkouts waiting on freed capacity. */
  private pump(): void {
    while (this.waiters.length > 0 && this.total < this.maxWorkers) {
      const waiter = this.waiters.shift()!
      this.spawn().then(
        (slot) => waiter.resolve(slot),
        (err) => waiter.reject(err instanceof Error ? err : new Error(String(err))),
      )
    }
  }

  /** Creates one worker, counting it against `total` (decremented on failure). */
  private async spawn(): Promise<WorkerSlot> {
    this.total++
    try {
      return { worker: await this.factory(), checkouts: 0 }
    } catch (err) {
      this.total--
      throw err
    }
  }
}
