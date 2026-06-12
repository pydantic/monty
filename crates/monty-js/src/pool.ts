// The worker pool: `Monty` owns a set of `monty --subprocess` children and
// hands them out one-per-session via `checkout()`. Sizing is elastic between
// `minProcesses` (prewarmed) and `maxProcesses` (spawned on demand); crashed
// or recycled workers are replaced transparently, so one sandbox crash never
// affects other sessions.

import { availableParallelism } from 'node:os'
import { create } from '@bufbuild/protobuf'
import { findMontyBinary } from './binary.js'
import { ReplCreateSchema, ResourceLimitsSchema, type ReplCreate } from './generated/monty/v1/monty_pb.js'
import { MontySession } from './session.js'
import { Worker } from './worker.js'

/** Options for [`Monty`]. */
export interface MontyOptions {
  /** Path to the `monty` binary; resolved automatically when omitted. */
  binaryPath?: string
  /** Workers spawned up front by `create()` (default 1). */
  minProcesses?: number
  /** Worker cap; checkouts beyond it wait (default: CPU count). */
  maxProcesses?: number
  /**
   * Seconds to wait for a free worker when the pool is exhausted before
   * `checkout()` rejects (default: wait forever).
   */
  checkoutTimeout?: number
  /**
   * Hard per-turn deadline in seconds: a worker that does not answer a
   * protocol request in time is killed and the session fails with
   * `MontyCrashedError` (`timedOut: true`). Off by default — prefer the
   * in-sandbox `maxDurationSecs` limit; this is the backstop for code that
   * wedges the interpreter itself.
   */
  requestTimeout?: number
  /**
   * Grace period in seconds for the automatic `maxDurationSecs` backstop
   * (default 1). For sessions checked out with a `maxDurationSecs` limit,
   * the worker reports its cumulative execution time on every protocol turn
   * (the sandbox clock runs only while the interpreter executes, never while
   * suspended waiting on the host) and the host kills the worker this long
   * after the remaining budget expires — covering cases where the in-sandbox
   * limit cannot fire, like a blocking syscall inside a mount. Surfaces as
   * `MontyCrashedError` (`timedOut: true`), losing the session. `null`
   * disables the backstop; `requestTimeout` applies independently.
   */
  durationLimitGrace?: number | null
  /** Recycle a worker (kill and replace) after serving this many sessions. */
  maxCheckoutsPerWorker?: number
}

/** Options for [`Monty.checkout`], mirroring `pydantic_monty`. */
export interface CheckoutOptions {
  /** Name used in type-checking diagnostics (default `'main.py'`). */
  scriptName?: string
  /** Resource limits enforced inside the worker for the whole session. */
  limits?: ResourceLimits
  /** Type-check each fed snippet before executing it (default false). */
  typeCheck?: boolean
  /** Stub file contents used by type checking. */
  typeCheckStubs?: string
}

/** Sandbox resource limits. Omitted fields mean "unlimited". */
export interface ResourceLimits {
  maxAllocations?: number
  maxDurationSecs?: number
  maxMemory?: number
  gcInterval?: number
  maxRecursionDepth?: number
}

/**
 * An async pool of crash-isolated `monty` worker subprocesses — the only way
 * this package runs Python. A worker that segfaults or is OOM-killed takes
 * down its own session only; the pool replaces it.
 *
 * ```ts
 * await using pool = await Monty.create()
 * await using session = await pool.checkout()
 * const result = await session.feedRun('1 + 1') // 2
 * ```
 */
export class Monty {
  private readonly binaryPath: string
  private readonly maxProcesses: number
  private readonly checkoutTimeoutMs: number | null
  private readonly requestTimeoutMs: number | null
  private readonly durationLimitGraceMs: number | null
  private readonly maxCheckoutsPerWorker: number | null
  private readonly idle: Worker[] = []
  /** Workers alive in any state (idle or checked out). */
  private total = 0
  /** FIFO of checkout() calls waiting for a worker to be released. */
  private readonly waiters: Array<(worker: Worker | null) => void> = []
  private closed = false

  private constructor(options: MontyOptions) {
    this.binaryPath = findMontyBinary(options.binaryPath)
    this.maxProcesses = options.maxProcesses ?? availableParallelism()
    this.checkoutTimeoutMs = options.checkoutTimeout !== undefined ? options.checkoutTimeout * 1000 : null
    this.requestTimeoutMs = options.requestTimeout !== undefined ? options.requestTimeout * 1000 : null
    this.durationLimitGraceMs =
      options.durationLimitGrace === undefined
        ? 1000
        : options.durationLimitGrace === null
          ? null
          : options.durationLimitGrace * 1000
    this.maxCheckoutsPerWorker = options.maxCheckoutsPerWorker ?? null
    if (this.maxProcesses < 1) {
      throw new Error('maxProcesses must be at least 1')
    }
  }

  /** Creates the pool and prewarms `minProcesses` workers. */
  static async create(options: MontyOptions = {}): Promise<Monty> {
    const pool = new Monty(options)
    const min = options.minProcesses ?? 1
    if (min > pool.maxProcesses) {
      throw new Error('minProcesses cannot exceed maxProcesses')
    }
    // allSettled so a partial prewarm failure can kill the workers that did
    // spawn — Promise.all would abandon them as orphan processes
    const spawned = await Promise.allSettled(Array.from({ length: min }, () => pool.spawnWorker()))
    const failed = spawned.find((result) => result.status === 'rejected')
    if (failed !== undefined) {
      for (const result of spawned) {
        if (result.status === 'fulfilled') {
          result.value.kill()
        }
      }
      throw failed.reason
    }
    pool.idle.push(...spawned.map((result) => (result as PromiseFulfilledResult<Worker>).value))
    return pool
  }

  /**
   * Checks a worker out of the pool (spawning one if allowed) and creates a
   * REPL session in it. Release the worker with `session.close()` (or
   * `await using`).
   */
  async checkout(options: CheckoutOptions = {}): Promise<MontySession> {
    const worker = await this.acquire()
    const durationBudgetMs =
      options.limits?.maxDurationSecs !== undefined ? options.limits.maxDurationSecs * 1000 : null
    const session = new MontySession(this, worker, this.requestTimeoutMs, durationBudgetMs, this.durationLimitGraceMs)
    try {
      await session.createRepl(buildReplCreate(options))
    } catch (err) {
      this.discard(worker)
      throw err
    }
    return session
  }

  /**
   * Shuts the pool down: idle workers exit and no new checkouts are
   * accepted. Sessions still checked out keep their workers until closed.
   */
  async close(): Promise<void> {
    if (this.closed) {
      return
    }
    this.closed = true
    for (const waiter of this.waiters.splice(0)) {
      waiter(null)
    }
    const idle = this.idle.splice(0)
    this.total -= idle.length
    await Promise.all(idle.map((w) => w.shutdown()))
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close()
  }

  /** Takes an idle worker, spawns a new one, or waits for a release. */
  private async acquire(): Promise<Worker> {
    if (this.closed) {
      throw new Error('the pool is closed — create a new Monty pool')
    }
    // Discard workers that died while idle (reaped transparently).
    for (;;) {
      const worker = this.idle.pop()
      if (worker === undefined) {
        break
      }
      if (worker.alive) {
        return worker
      }
      this.total -= 1
    }
    if (this.total < this.maxProcesses) {
      return await this.spawnWorker()
    }
    return await this.waitForWorker()
  }

  /** Blocks until a checked-out worker is released, honouring the timeout. */
  private waitForWorker(): Promise<Worker> {
    return new Promise((resolve, reject) => {
      let timer: NodeJS.Timeout | null = null
      const waiter = (worker: Worker | null) => {
        if (timer !== null) {
          clearTimeout(timer)
        }
        if (worker === null) {
          reject(new Error(this.closed ? 'the pool is closed — create a new Monty pool' : 'checkout timed out'))
        } else {
          resolve(worker)
        }
      }
      if (this.checkoutTimeoutMs !== null) {
        timer = setTimeout(() => {
          const i = this.waiters.indexOf(waiter)
          if (i !== -1) {
            this.waiters.splice(i, 1)
          }
          reject(new Error(`no worker became available within ${this.checkoutTimeoutMs! / 1000}s`))
        }, this.checkoutTimeoutMs)
      }
      this.waiters.push(waiter)
    })
  }

  private async spawnWorker(): Promise<Worker> {
    this.total += 1
    try {
      return await Worker.spawn(this.binaryPath)
    } catch (err) {
      this.total -= 1
      throw err
    }
  }

  /**
   * Returns a worker to the idle queue after a clean `finish()`. Recycles
   * (kills) it when past its checkout budget or when the pool has closed.
   */
  release(worker: Worker): void {
    worker.checkoutsServed += 1
    const expired = this.maxCheckoutsPerWorker !== null && worker.checkoutsServed >= this.maxCheckoutsPerWorker
    if (this.closed || expired || !worker.alive) {
      this.discard(worker)
      return
    }
    const waiter = this.waiters.shift()
    if (waiter !== undefined) {
      waiter(worker)
    } else {
      this.idle.push(worker)
    }
  }

  /** Drops a dead/poisoned worker; a queued waiter gets a fresh one instead. */
  discard(worker: Worker): void {
    worker.kill()
    this.total -= 1
    const waiter = this.waiters.shift()
    if (waiter !== undefined) {
      this.spawnWorker().then(
        (fresh) => {
          // the pool may have closed while the replacement was spawning; a
          // worker handed out now would never be shut down
          if (this.closed) {
            fresh.kill()
            this.total -= 1
            waiter(null)
          } else {
            waiter(fresh)
          }
        },
        () => waiter(null),
      )
    }
  }
}

/** Builds the `ReplCreate` request from checkout options. */
function buildReplCreate(options: CheckoutOptions): ReplCreate {
  const limits = options.limits ?? {}
  return create(ReplCreateSchema, {
    scriptName: options.scriptName ?? 'main.py',
    limits: create(ResourceLimitsSchema, {
      ...(limits.maxAllocations !== undefined ? { maxAllocations: BigInt(limits.maxAllocations) } : {}),
      ...(limits.maxDurationSecs !== undefined
        ? { maxDurationMicros: BigInt(Math.round(limits.maxDurationSecs * 1_000_000)) }
        : {}),
      ...(limits.maxMemory !== undefined ? { maxMemoryBytes: BigInt(limits.maxMemory) } : {}),
      ...(limits.gcInterval !== undefined ? { gcInterval: BigInt(limits.gcInterval) } : {}),
      ...(limits.maxRecursionDepth !== undefined ? { maxRecursionDepth: BigInt(limits.maxRecursionDepth) } : {}),
    }),
    typeCheck: options.typeCheck ?? false,
    ...(options.typeCheckStubs !== undefined ? { typeCheckStubs: options.typeCheckStubs } : {}),
  })
}
