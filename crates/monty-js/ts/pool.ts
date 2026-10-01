// The worker pool: `Monty` owns a set of `monty subprocess` children and
// hands them out one-per-session via `checkout()`. The pool, watchdogs and
// crash recovery live in the native `monty-pool` crate (shared with
// pydantic_monty); this class normalises options, resolves the worker
// binary, and wraps the native classes in the public API.

import { availableParallelism } from 'node:os'
import { NativePool } from '../native-addon.js'
import { findMontyBinary } from './binary.js'
import {
  type AssertMessageAnnotations,
  type OsPolicy,
  type EncodedOsPolicy,
  type TypeCheckFormat,
  encodeAssertMessageAnnotations,
  encodeOsPolicy,
  validateMaxCheckouts,
} from './options.js'
import { MontySession } from './session.js'
import { captureTelemetryContext } from './telemetry.js'

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
   * in-sandbox `maxFeedDurationSecs` limit; this is the backstop for code that
   * wedges the interpreter itself.
   */
  requestTimeout?: number
  /**
   * Grace period in seconds for the automatic `maxFeedDurationSecs` backstop
   * (default 1, `null` disables). For sessions with a `maxFeedDurationSecs`
   * limit, the worker reports the running feed's execution time each turn (the
   * sandbox clock runs only while the interpreter executes, never while
   * suspended on the host) and the host kills the worker this long after the
   * budget expires — covering cases the in-sandbox limit cannot catch (its
   * check only runs at interpreter checkpoints). Surfaces as `MontyCrashedError`
   * (`timedOut: true`), losing the session. `requestTimeout` is independent.
   */
  feedDurationLimitGrace?: number | null
  /**
   * As `feedDurationLimitGrace`, but for `maxTurnDurationSecs`: the host kills the
   * worker this long after the current turn's budget expires (default 1,
   * `null` disables).
   */
  turnDurationLimitGrace?: number | null
  /** Recycle a worker after this many sessions: an integer from 0 to 4294967295.
   *  Both 0 and 1 retire after each checkout; omitted means no recycling. */
  maxCheckoutsPerWorker?: number
}

/** Options for [`Monty.checkout`], mirroring `pydantic_monty`. */
export interface CheckoutOptions {
  /**
   * Name used in tracebacks and type-checking diagnostics (default
   * `'main.py'`), and the basis of the sandbox's `__file__`: its final path
   * component placed under the working directory a feed starts in
   * (`/main.py` at the root).
   */
  scriptName?: string
  /** Resource limits enforced inside the worker for the whole session. */
  limits?: ResourceLimits
  /** Type-check each fed snippet before executing it (default false). */
  typeCheck?: boolean
  /** Stub file contents used by type checking. */
  typeCheckStubs?: string
  /**
   * How `MontyTypingError` diagnostics are rendered (default `'full'`).
   * Chosen here rather than on the thrown error because the checker's
   * structured diagnostics never leave the worker.
   */
  typeCheckFormat?: TypeCheckFormat
  /**
   * Render typing diagnostics with ANSI colour escapes (default false); only
   * `'full'` and `'concise'` carry colour.
   */
  typeCheckColor?: boolean
  /**
   * Give failed `assert` statements pytest-style introspected messages, e.g.
   * `AssertionError: assert 2 == 5` — a deliberate divergence from CPython's
   * empty bare `AssertionError` (see limitations/assert.md). Default true; set
   * false to disable annotations, or an integer >= 1 to customize the
   * per-operand repr truncation length (default 120 bytes).
   */
  assertMessageAnnotations?: AssertMessageAnnotations
  /**
   * How long, in seconds, the worker may hold buffered `print()` output before
   * sending it, so a burst of prints costs one `printCallback` call rather
   * than one each (default 0.005). `0` restores line buffering, delivering
   * each completed line on its own. Output is always flushed before a host
   * call and before a feed ends, so this only sets how far live output may
   * lag — never what arrives, or in what order.
   */
  printFlushInterval?: number
  /**
   * Session clock, sleep, process-clock and random initialization policies; see `OsPolicy`.
   * Defaults to the worker's clock in UTC and its entropy, with pool-managed sleeps capped at ten seconds.
   * Sleeps count toward suspensions and `maxTotalSleepSecs`, but not execution duration limits.
   */
  osPolicy?: OsPolicy
}

/**
 * Sandbox resource limits. Omitted fields are unlimited except
 * `maxRecursionDepth` and `maxSuspensions`, which keep their 1000 defaults.
 * The pool counts `maxSuspensions` per checkout and aborts an over-budget
 * feed with an uncatchable `RuntimeError`.
 *
 * Both duration limits share one clock, which runs only while sandboxed
 * code executes, never while suspended on the host; they differ in when it
 * restarts: at each feed, at each host round trip. Exceeding either raises
 * `TimeoutError` in the sandbox.
 */
export interface ResourceLimits {
  /**
   * @deprecated Removed: it capped a whole session, which neither replacement
   * does, so there is no value to carry over. Pick `maxFeedDurationSecs` or
   * `maxTurnDurationSecs`. Declared `never` so a stale key still fails to
   * compile rather than being silently dropped at the boundary.
   */
  maxDurationSecs?: never
  /** Maximum execution time for a single feed (`feedRun` or `feedStart`). */
  maxFeedDurationSecs?: number
  /** Maximum execution time between host round trips. */
  maxTurnDurationSecs?: number
  maxMemory?: number
  gcInterval?: number
  maxRecursionDepth?: number
  maxSuspensions?: number
  /**
   * Maximum cumulative seconds of `'system'` sleep, excluded from execution duration limits.
   * The pool charges each sleep before waiting; exceeding the limit raises an uncatchable `TimeoutError`.
   */
  maxTotalSleepSecs?: number
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
    validateMaxCheckouts(options.maxCheckoutsPerWorker)
    const native = new NativePool({
      binaryPath: findMontyBinary(options.binaryPath),
      minProcesses: options.minProcesses ?? 1,
      maxProcesses: options.maxProcesses ?? availableParallelism(),
      ...(options.checkoutTimeout !== undefined ? { checkoutTimeoutMs: options.checkoutTimeout * 1000 } : {}),
      ...(options.requestTimeout !== undefined ? { requestTimeoutMs: options.requestTimeout * 1000 } : {}),
      // `null` disables a backstop; omitted means the 1s default
      ...graceMs('feedDurationLimitGraceMs', options.feedDurationLimitGrace),
      ...graceMs('turnDurationLimitGraceMs', options.turnDurationLimitGrace),
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
    const osPolicy = encodeOsPolicy(options.osPolicy ?? {})
    const native = this.native.checkout({
      scriptName: options.scriptName ?? 'main.py',
      ...(options.limits !== undefined ? { limits: options.limits } : {}),
      typeCheck: options.typeCheck ?? false,
      ...(options.typeCheckStubs !== undefined ? { typeCheckStubs: options.typeCheckStubs } : {}),
      ...(options.typeCheckFormat !== undefined ? { typeCheckFormat: options.typeCheckFormat } : {}),
      ...(options.typeCheckColor !== undefined ? { typeCheckColor: options.typeCheckColor } : {}),
      ...(assertAnnotations !== undefined ? { assertMessageAnnotations: assertAnnotations } : {}),
      ...(options.printFlushInterval !== undefined ? { printFlushIntervalMs: options.printFlushInterval * 1000 } : {}),
      ...nativeOsPolicy(osPolicy),
    })
    const telemetryContext = captureTelemetryContext()
    await native.enter(telemetryContext)
    return new MontySession(native)
  }

  /**
   * Shuts the pool down: idle workers exit and pending/new checkouts reject.
   * Sessions already checked out keep their workers until closed.
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

/**
 * Renders one duration-backstop grace as the native option the pool takes:
 * `null` yields no key at all, and an absent grace falls back to 1s.
 */
function graceMs(key: string, seconds: number | null | undefined): Record<string, number> {
  return seconds === null ? {} : { [key]: (seconds ?? 1) * 1000 }
}

/** Flattens normalized options into native binding fields. */
function nativeOsPolicy(calls: EncodedOsPolicy): Record<string, unknown> {
  const fields: Record<string, unknown> = {}
  if (typeof calls.datetime === 'string') {
    fields.datetimeKind = calls.datetime
  } else if (calls.datetime !== undefined) {
    fields.datetimeKind = 'fixed'
    fields.datetimeUnixSeconds = calls.datetime.unixSeconds
    fields.datetimeMicrosecond = calls.datetime.microsecond
  }
  if (calls.timezone === 'utc') {
    fields.timezoneKind = 'utc'
  } else if (typeof calls.timezone === 'string') {
    fields.timezoneKind = 'named'
    fields.timezoneName = calls.timezone
  } else if (calls.timezone !== undefined) {
    fields.timezoneKind = 'fixed'
    fields.timezoneOffsetSeconds = calls.timezone.offsetSeconds
    if (calls.timezone.name !== undefined) fields.timezoneName = calls.timezone.name
  }
  if (calls.sleep !== undefined) fields.sleep = calls.sleep
  if (calls.sleepSystemMaxSecs !== undefined) fields.sleepSystemMaxSecs = calls.sleepSystemMaxSecs
  if (calls.randomStart === 'call_host') {
    fields.randomStartKind = 'call_host'
  } else if (calls.randomStart !== undefined) {
    fields.randomStartKind = 'seed'
    const seed = calls.randomStart.seed
    if ('int' in seed) fields.randomSeedInt = Buffer.from(seed.int)
    else if ('float' in seed) fields.randomSeedFloat = seed.float
    else if ('str' in seed) fields.randomSeedStr = seed.str
    else fields.randomSeedBytes = Buffer.from(seed.bytes)
  }
  if (calls.processTime !== undefined) fields.processTime = calls.processTime
  return fields
}
