// One checked-out worker driving one REPL session. `feedRun` is the drive
// loop: it runs protocol turns through the native binding (which owns the
// pool, framing, watchdogs and value conversion) and answers the suspension
// events each turn resolves to — external function calls, OS callbacks, name
// lookups, async futures — until the turn completes, mirroring
// pydantic_monty's AsyncMontySession.
//
// External functions may return promises: the call is registered as an
// external future so other sandbox tasks keep executing, and results are
// delivered when the worker reports everything is blocked (`resolveFutures`).

import type { NativeSession } from '../index.js'
import { MontyCrashedError, MontyError, montyErrorFromNative, MontyTypingError, ProtocolError } from './errors.js'
import { PYTHON_EXC_NAMES } from './errors.js'
import { mountsToNative, type MountDir } from './mount.js'
import type { FunctionCallTurn, NativeFutureResult, NativeTurn, OsCallTurn, ResolveFuturesTurn } from './native.js'

/**
 * Sentinel an `os` callback returns to decline a call: the sandbox then
 * raises the call's default exception (e.g. `PermissionError` for
 * filesystem access), exactly as if no callback existed.
 */
export const NOT_HANDLED: unique symbol = Symbol('NOT_HANDLED')

/** An external function: sync or async, called with the sandbox's args. */
export type ExternalFunction = (...args: never[]) => unknown

/**
 * Handler for OS calls (e.g. `Path.read_text`, `os.getenv`) that no mount
 * covered. Return a value, a promise, or [`NOT_HANDLED`].
 */
export type OsCallback = (name: string, args: unknown[], kwargs: Record<string, unknown>) => unknown

/** Receives sandbox `print()` output (line-buffered). */
export type PrintCallback = (stream: 'stdout' | 'stderr', text: string) => void

/** Options for [`MontySession.feedRun`]. */
export interface FeedOptions {
  /** Values bound as globals before the snippet runs. */
  inputs?: Record<string, unknown>
  /** Host functions the sandbox may call by name. */
  externalFunctions?: Record<string, ExternalFunction>
  /** Receives `print()` output; defaults to the host process stdout/stderr. */
  printCallback?: PrintCallback
  /** Host directories mounted into the sandbox for this feed. */
  mount?: MountDir | MountDir[]
  /** Handler for OS calls not covered by mounts. */
  os?: OsCallback
  /** Skip type checking for this feed even when the session enables it. */
  skipTypeCheck?: boolean
}

/** A promise-returning external call registered as a sandbox future. */
interface PendingFuture {
  readonly callId: number
  done: boolean
  outcome: { ok: unknown } | { err: unknown } | null
  /** Settles (never rejects) when the underlying promise settles. */
  readonly settled: Promise<void>
}

/**
 * One worker process dedicated to one REPL session; created by
 * [`Monty.checkout`]. Session state (globals, functions) persists across
 * `feedRun` calls. Close it (or `await using`) to return the worker to the
 * pool.
 */
export class MontySession {
  private readonly native: NativeSession
  /** Set once the session is unusable: crashed worker or protocol error. */
  private broken: Error | null = null
  private closed = false
  /** Pending async external calls, by call id. */
  private readonly futures = new Map<number, PendingFuture>()

  /** @internal — sessions are created by `Monty.checkout`. */
  constructor(native: NativeSession) {
    this.native = native
  }

  /**
   * Executes one snippet in the worker, driving external function calls
   * (which may return promises), OS callbacks, and print callbacks in this
   * process. Returns the snippet's trailing expression value.
   */
  async feedRun(code: string, options: FeedOptions = {}): Promise<unknown> {
    this.ensureUsable()
    const printTarget = new PrintTarget(options.printCallback)
    const onPrint = printTarget.write.bind(printTarget)
    try {
      let turn = (await this.native.feed(
        code,
        options.inputs ?? null,
        mountsToNative(options.mount),
        options.skipTypeCheck ?? false,
        onPrint,
      )) as NativeTurn
      for (;;) {
        switch (turn.kind) {
          case 'complete':
            printTarget.throwIfFailed()
            return turn.value
          case 'error':
            printTarget.throwIfFailed()
            throw montyErrorFromNative(turn.exception)
          case 'typingError':
            printTarget.throwIfFailed()
            throw new MontyTypingError(turn.diagnostics)
          case 'crashed':
            throw this.poison(new MontyCrashedError(turn.message, turn))
          case 'protocol':
            throw this.poison(new ProtocolError(turn.message))
        }
        turn = await this.answer(turn, options, onPrint)
      }
    } finally {
      // failed feeds abandon their futures too — without this, entries for
      // promises the worker will never ask about again accumulate
      this.futures.clear()
    }
  }

  /**
   * Serializes the worker's session state into opaque bytes via monty's dump
   * format. The session stays usable; the bytes can only be restored by a
   * monty worker of the same version.
   */
  async dump(): Promise<Buffer> {
    this.ensureUsable()
    return Buffer.from(await this.native.dump())
  }

  /**
   * OS process id of this session's worker, or `undefined` when no worker is
   * attached or a turn is in flight on this session (diagnostics/tests).
   */
  get workerPid(): number | undefined {
    return this.native.workerPid ?? undefined
  }

  /**
   * Ends the session and returns the worker to the pool. A crashed or
   * poisoned worker has already been discarded and replaced.
   */
  async close(): Promise<void> {
    if (this.closed) {
      return
    }
    this.closed = true
    await this.native.finish()
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close()
  }

  /** Answers one suspension turn and runs the resume turn it produces. */
  private async answer(
    turn: FunctionCallTurn | OsCallTurn | ResolveFuturesTurn | { kind: 'nameLookup'; name: string },
    options: FeedOptions,
    onPrint: PrintCallback,
  ): Promise<NativeTurn> {
    let next: Promise<object>
    try {
      switch (turn.kind) {
        case 'functionCall':
          next = this.answerFunctionCall(turn, options.externalFunctions, onPrint)
          break
        case 'osCall':
          next = this.answerOsCall(turn, options.os, onPrint)
          break
        case 'nameLookup': {
          const fn = options.externalFunctions?.[turn.name]
          next = this.native.resumeNameLookup(fn === undefined ? null : fn.name || '<anonymous>', onPrint)
          break
        }
        case 'resolveFutures':
          next = this.answerResolveFutures(turn, onPrint)
          break
      }
      return (await next) as NativeTurn
    } catch (err) {
      // A handler that throws instead of answering leaves the worker
      // suspended, awaiting a resume that will never come — the session
      // cannot be trusted any more.
      this.broken ??= err instanceof Error ? err : new Error(String(err))
      throw err
    }
  }

  /** Calls the matching external function and resumes with its result. */
  private answerFunctionCall(
    call: FunctionCallTurn,
    externalFunctions: Record<string, ExternalFunction> | undefined,
    onPrint: PrintCallback,
  ): Promise<object> {
    if (call.methodCall) {
      // Dataclass method dispatch needs host-side class objects, which this
      // package has no registry for (unlike pydantic_monty).
      return this.native.resumeError(
        'RuntimeError',
        `method calls on host objects are not supported: ${call.functionName}`,
        onPrint,
      )
    }
    const fn = externalFunctions?.[call.functionName]
    if (fn === undefined) {
      return this.native.resumeNotFound(onPrint)
    }
    let returned: unknown
    try {
      returned = fn(...(buildCallArgs(call) as never[]))
    } catch (err) {
      const { excType, message } = jsErrorParts(err)
      return this.native.resumeError(excType, message, onPrint)
    }
    if (isThenable(returned)) {
      this.registerFuture(call.callId, Promise.resolve(returned))
      return this.native.resumeFuture(onPrint)
    }
    return this.native.resumeReturn(returned, onPrint)
  }

  /** Dispatches an OS call to the `os` callback (or its default error). */
  private async answerOsCall(call: OsCallTurn, os: OsCallback | undefined, onPrint: PrintCallback): Promise<object> {
    if (os === undefined) {
      return await this.native.resumeNotHandled(onPrint)
    }
    let returned: unknown
    try {
      returned = os(call.functionName, call.args, kwargsToRecord(call.kwargs))
      if (isThenable(returned)) {
        returned = await returned
      }
    } catch (err) {
      const { excType, message } = jsErrorParts(err)
      return await this.native.resumeError(excType, message, onPrint)
    }
    if (returned === NOT_HANDLED) {
      return await this.native.resumeNotHandled(onPrint)
    }
    return await this.native.resumeReturn(returned, onPrint)
  }

  /** Tracks a promise so `resolveFutures` can later deliver its outcome. */
  private registerFuture(callId: number, promise: Promise<unknown>): void {
    const future: { -readonly [K in keyof PendingFuture]: PendingFuture[K] } = {
      callId,
      done: false,
      outcome: null,
      settled: undefined as unknown as Promise<void>,
    }
    future.settled = promise.then(
      (ok) => {
        future.done = true
        future.outcome = { ok }
      },
      (err: unknown) => {
        future.done = true
        future.outcome = { err }
      },
    )
    this.futures.set(callId, future)
  }

  /**
   * Every sandbox task is blocked: wait until at least one pending future
   * settles, then deliver everything that is ready.
   */
  private async answerResolveFutures(event: ResolveFuturesTurn, onPrint: PrintCallback): Promise<object> {
    const pending = event.pendingCallIds.map((id) => {
      const future = this.futures.get(id)
      if (future === undefined) {
        throw new ProtocolError(`worker reported unknown pending call id ${id}`)
      }
      return future
    })
    if (pending.length === 0) {
      throw new ProtocolError('worker reported ResolveFutures with no pending call ids')
    }
    await Promise.race(pending.map((f) => f.settled))
    const results: NativeFutureResult[] = pending
      .filter((f) => f.done)
      .map((f) => {
        this.futures.delete(f.callId)
        const outcome = f.outcome!
        if ('ok' in outcome) {
          return { callId: f.callId, ok: true, value: outcome.ok }
        }
        const { excType, message } = jsErrorParts(outcome.err)
        return { callId: f.callId, ok: false, excType, message }
      })
    return await this.native.resolveFutures(results, onPrint)
  }

  /** Poisons the session over a worker death or protocol violation. */
  private poison(err: Error): Error {
    this.broken = err
    return err
  }

  private ensureUsable(): void {
    if (this.closed) {
      throw new Error('the session is closed — check out a new one')
    }
    if (this.broken !== null) {
      throw this.broken
    }
  }
}

/** A new `PrintTarget` per feed: routes prints, capturing callback failures. */
class PrintTarget {
  private readonly callback: PrintCallback | undefined
  private failure: unknown = null

  constructor(callback: PrintCallback | undefined) {
    this.callback = callback
  }

  write(stream: 'stdout' | 'stderr', text: string): void {
    if (this.failure !== null) {
      return
    }
    if (this.callback === undefined) {
      ;(stream === 'stdout' ? process.stdout : process.stderr).write(text)
      return
    }
    try {
      this.callback(stream, text)
    } catch (err) {
      // Captured and re-thrown at the turn boundary: this function is called
      // from the native binding's threadsafe-function bridge, where a throw
      // would be an unhandled error rather than failing the feed.
      this.failure = err
    }
  }

  /** Print failures take precedence over the turn's own outcome. */
  throwIfFailed(): void {
    if (this.failure !== null) {
      throw this.failure
    }
  }
}

/** Positional args, with kwargs appended as an object when present. */
function buildCallArgs(call: FunctionCallTurn): unknown[] {
  const args = [...call.args]
  if (call.kwargs.length > 0) {
    args.push(kwargsToRecord(call.kwargs))
  }
  return args
}

/**
 * Converts `[key, value]` kwarg pairs into a record (string keys only). The
 * record has a null prototype: keys are sandbox-controlled, and assigning a
 * key like `__proto__` to a normal object would replace its prototype
 * instead of creating a property.
 */
function kwargsToRecord(pairs: [unknown, unknown][]): Record<string, unknown> {
  const kwargs: Record<string, unknown> = Object.create(null)
  for (const [key, value] of pairs) {
    if (typeof key === 'string') {
      kwargs[key] = value
    }
  }
  return kwargs
}

/**
 * Maps a thrown JS value to the exception the sandbox re-raises. The JS
 * error's `name` is used when it matches a Python exception type (Python
 * code can catch `TypeError` from a JS `TypeError`); anything else becomes
 * `RuntimeError`.
 */
function jsErrorParts(err: unknown): { excType: string; message: string } {
  if (err instanceof MontyError) {
    const { typeName, message } = err.exception
    return { excType: typeName, message }
  }
  if (err instanceof Error) {
    const excType = PYTHON_EXC_NAMES.has(err.name) ? err.name : 'RuntimeError'
    return { excType, message: err.message }
  }
  return { excType: 'RuntimeError', message: String(err) }
}

function isThenable(value: unknown): value is PromiseLike<unknown> {
  return typeof value === 'object' && value !== null && typeof (value as { then?: unknown }).then === 'function'
}
