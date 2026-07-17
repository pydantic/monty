// The worker pool: `Monty` owns a set of `monty subprocess` children and
// hands them out one-per-session via `checkout()`. The pool, watchdogs and
// crash recovery live in the native `monty-pool` crate (shared with
// pydantic_monty); this class normalises options, resolves the worker
// binary, and wraps the native classes in the public API.

import { availableParallelism } from 'node:os'
import { NativePool } from '../native-addon.js'
import { findMontyBinary } from './binary.js'
import { type AssertMessageAnnotations, encodeAssertMessageAnnotations } from './options.js'
import { MontySession } from './session.js'

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
   * (default 1, `null` disables). For sessions with a `maxDurationSecs`
   * limit, the worker reports cumulative execution time each turn (the
   * sandbox clock runs only while the interpreter executes, never while
   * suspended on the host) and the host kills the worker this long after the
   * budget expires — covering cases the in-sandbox limit cannot catch, like a
   * blocking syscall inside a mount. Surfaces as `MontyCrashedError`
   * (`timedOut: true`), losing the session. `requestTimeout` is independent.
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
  /**
   * Give failed `assert` statements pytest-style introspected messages, e.g.
   * `AssertionError: assert 2 == 5` — a deliberate divergence from CPython's
   * empty bare `AssertionError` (see limitations/assert.md). Default true; set
   * false to disable annotations, or an integer >= 1 to customize the
   * per-operand repr truncation length (default 120 bytes).
   */
  assertMessageAnnotations?: AssertMessageAnnotations
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
 * An async pool of crash-isolated `monty` worker subprocesses — the primary
 * way this package runs Python. A worker that segfaults or is OOM-killed
 * takes down its own session only; the pool replaces it.
 *
 * ```ts
 * await using pool = await Monty.create()
 * await using session = await pool.checkout()
 * const result = await session.feedRun('1 + 1') // 2
 * ```
 */
export class Monty {
  private readonly native: NativePool
  private closed = false

  private constructor(native: NativePool) {
    this.native = native
  }

  /** Creates the pool and prewarms `minProcesses` workers. */
  static async create(options: MontyOptions = {}): Promise<Monty> {
    const native = new NativePool({
      binaryPath: findMontyBinary(options.binaryPath),
      minProcesses: options.minProcesses ?? 1,
      maxProcesses: options.maxProcesses ?? availableParallelism(),
      ...(options.checkoutTimeout !== undefined ? { checkoutTimeoutMs: options.checkoutTimeout * 1000 } : {}),
      ...(options.requestTimeout !== undefined ? { requestTimeoutMs: options.requestTimeout * 1000 } : {}),
      // `null` disables the backstop; omitted means the 1s default
      ...(options.durationLimitGrace !== null
        ? { durationLimitGraceMs: (options.durationLimitGrace ?? 1) * 1000 }
        : {}),
      ...(options.maxCheckoutsPerWorker !== undefined ? { maxCheckoutsPerWorker: options.maxCheckoutsPerWorker } : {}),
    })
    await native.start()
    return new Monty(native)
  }

  /**
   * Checks a worker out of the pool (spawning one if allowed) and creates a
   * REPL session in it. Release the worker with `session.close()` (or
   * `await using`).
   */
  async checkout(options: CheckoutOptions = {}): Promise<MontySession> {
    if (this.closed) {
      throw new Error('the pool is closed — create a new Monty pool')
    }
    const assertAnnotations = encodeAssertMessageAnnotations(options.assertMessageAnnotations)
    const native = this.native.checkout({
      scriptName: options.scriptName ?? 'main.py',
      ...(options.limits !== undefined ? { limits: options.limits } : {}),
      typeCheck: options.typeCheck ?? false,
      ...(options.typeCheckStubs !== undefined ? { typeCheckStubs: options.typeCheckStubs } : {}),
      ...(assertAnnotations !== undefined ? { assertMessageAnnotations: assertAnnotations } : {}),
    })
    await native.enter()
    return new MontySession(native)
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
    await this.native.close()
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close()
  }
}
