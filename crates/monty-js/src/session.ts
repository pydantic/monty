// One checked-out worker driving one REPL session. `feedRun` implements the
// drive loop: it sends a snippet and answers the worker's suspension events
// (external function calls, OS callbacks, name lookups, async futures) until
// the turn completes, mirroring pydantic_monty's AsyncMontySession.
//
// External functions may return promises: the call is registered as an
// external future so other sandbox tasks keep executing, and results are
// delivered when the worker reports that everything is blocked
// (`ResolveFutures`).

import { create } from '@bufbuild/protobuf'
import { ConversionError, exceedsMaxValueDepth, jsToMonty, montyToJs } from './convert.js'
import { MontyError, montyErrorFromProto, MontyTypingError, PYTHON_EXC_NAMES } from './errors.js'
import {
  DumpSchema,
  ExtResultSchema,
  FutureResultSchema,
  MontyErrorSchema,
  NamedValueSchema,
  ResetSchema,
  ResumeCallSchema,
  ResumeFuturesSchema,
  ResumeNameLookupSchema,
  ReplFeedSchema,
  UnitSchema,
  type Event,
  type ExtResult,
  type FunctionCall,
  type MontyError as PbMontyError,
  type OsCall,
  type ReplCreate,
  type Request,
  type ResolveFutures,
} from './generated/monty/v1/monty_pb.js'
import { mountsToProto, type MountDir } from './mount.js'
import type { Monty } from './pool.js'
import { ProtocolError, type Worker } from './worker.js'

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
  private readonly pool: Monty
  private readonly worker: Worker
  private readonly requestTimeoutMs: number | null
  /** The session's `maxDurationSecs` budget in ms, for the host backstop. */
  private readonly durationBudgetMs: number | null
  /** Grace added to the remaining budget; `null` disables the backstop. */
  private readonly durationGraceMs: number | null
  /**
   * Cumulative sandbox execution time in ms as last reported by the worker —
   * the worker's clock is the single source of truth (it runs only while the
   * interpreter executes, never during suspensions or between feeds). It is
   * stamped on every turn-ending event and only ever ratchets up here, so a
   * compromised worker cannot rewind the host's view of its consumed budget.
   */
  private reportedExecutionMs = 0
  /** Set once the session is unusable: crashed worker or protocol error. */
  private broken: Error | null = null
  private closed = false
  /** Pending async external calls, by call id. */
  private readonly futures = new Map<number, PendingFuture>()

  /** @internal — sessions are created by `Monty.checkout`. */
  constructor(
    pool: Monty,
    worker: Worker,
    requestTimeoutMs: number | null,
    durationBudgetMs: number | null,
    durationGraceMs: number | null,
  ) {
    this.pool = pool
    this.worker = worker
    this.requestTimeoutMs = requestTimeoutMs
    this.durationBudgetMs = durationBudgetMs
    this.durationGraceMs = durationGraceMs
  }

  /** @internal — sends `ReplCreate` and awaits the ack. */
  async createRepl(replCreate: ReplCreate): Promise<void> {
    const event = await this.turn({ case: 'replCreate', value: replCreate }, null)
    if (event.kind.case !== 'ok') {
      throw this.unexpected(event, 'ReplCreate')
    }
  }

  /**
   * Executes one snippet in the worker, driving external function calls
   * (which may return promises), OS callbacks, and print callbacks in this
   * process. Returns the snippet's trailing expression value.
   */
  async feedRun(code: string, options: FeedOptions = {}): Promise<unknown> {
    this.ensureUsable()
    const inputs = Object.entries(options.inputs ?? {}).map(([name, js]) => {
      const value = convertInput(js)
      return create(NamedValueSchema, { name, value })
    })
    const feed = create(ReplFeedSchema, {
      code,
      inputs,
      mounts: mountsToProto(options.mount),
      skipTypeCheck: options.skipTypeCheck ?? false,
    })
    const printTarget = new PrintTarget(options.printCallback)
    try {
      let event = await this.turn({ case: 'replFeed', value: feed }, printTarget)
      for (;;) {
        const kind = event.kind
        switch (kind.case) {
          case 'complete':
            printTarget.throwIfFailed()
            return kind.value.value === undefined ? null : montyToJs(kind.value.value)
          case 'error':
            printTarget.throwIfFailed()
            throw montyErrorFromProto(kind.value.exception ?? missingField('Error.exception'))
          case 'typingError':
            printTarget.throwIfFailed()
            throw new MontyTypingError(kind.value.diagnostics)
        }
        let resume: Request['kind']
        try {
          resume = await this.buildResume(event, options)
        } catch (err) {
          // A handler that throws instead of answering leaves the worker
          // suspended, awaiting a resume that will never come — the session
          // cannot be trusted any more.
          this.broken ??= err instanceof Error ? err : new Error(String(err))
          throw err
        }
        event = await this.turn(resume, printTarget)
      }
    } finally {
      // failed feeds abandon their futures too — without this, entries for
      // promises the worker will never ask about again accumulate
      this.futures.clear()
    }
  }

  /** Builds the resume request answering one suspension event. */
  private async buildResume(event: Event, options: FeedOptions): Promise<Request['kind']> {
    const kind = event.kind
    switch (kind.case) {
      case 'functionCall':
        return await this.handleFunctionCall(kind.value, options.externalFunctions)
      case 'osCall':
        return await this.handleOsCall(kind.value, options.os)
      case 'nameLookup': {
        const fn = options.externalFunctions?.[kind.value.name]
        return {
          case: 'resumeNameLookup',
          value: create(ResumeNameLookupSchema, {
            kind:
              fn === undefined
                ? { case: 'undefined', value: create(UnitSchema) }
                : { case: 'value', value: jsToMonty(fn) },
          }),
        }
      }
      case 'resolveFutures':
        return await this.handleResolveFutures(kind.value)
      default:
        throw this.unexpected(event, 'ReplFeed')
    }
  }

  /**
   * Serializes the worker's session state into opaque bytes via monty's dump
   * format. The session stays usable; the bytes can only be restored by a
   * monty worker of the same version.
   */
  async dump(): Promise<Buffer> {
    this.ensureUsable()
    const event = await this.turn({ case: 'dump', value: create(DumpSchema) }, null)
    if (event.kind.case !== 'dumpResult') {
      throw this.unexpected(event, 'Dump')
    }
    return Buffer.from(event.kind.value.state)
  }

  /** OS process id of this session's worker (diagnostics/tests). */
  get workerPid(): number | undefined {
    return this.worker.pid
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
    if (this.broken !== null || !this.worker.alive) {
      this.pool.discard(this.worker)
      return
    }
    try {
      const event = await this.turn({ case: 'reset', value: create(ResetSchema) }, null)
      if (event.kind.case !== 'ok') {
        throw this.unexpected(event, 'Reset')
      }
      this.pool.release(this.worker)
    } catch {
      // Best effort: a worker that cannot reset cleanly is discarded.
      this.pool.discard(this.worker)
    }
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close()
  }

  /** Calls the matching external function and builds the resume request. */
  private async handleFunctionCall(
    call: FunctionCall,
    externalFunctions: Record<string, ExternalFunction> | undefined,
  ): Promise<Request['kind']> {
    let result: ExtResult['kind']
    if (call.methodCall) {
      // Dataclass method dispatch needs host-side class objects, which this
      // package has no registry for (unlike pydantic_monty).
      result = {
        case: 'error',
        value: pbError('RuntimeError', `method calls on host objects are not supported: ${call.functionName}`),
      }
    } else {
      const fn = externalFunctions?.[call.functionName]
      if (fn === undefined) {
        result = { case: 'notFound', value: call.functionName }
      } else {
        result = this.callExternal(fn, call)
      }
    }
    return {
      case: 'resumeCall',
      value: create(ResumeCallSchema, { callId: call.callId, result: create(ExtResultSchema, { kind: result }) }),
    }
  }

  /** Invokes one external function, registering promises as futures. */
  private callExternal(fn: ExternalFunction, call: FunctionCall): ExtResult['kind'] {
    let returned: unknown
    try {
      returned = fn(...(buildCallArgs(call) as never[]))
    } catch (err) {
      return { case: 'error', value: jsErrorToProto(err) }
    }
    if (isThenable(returned)) {
      this.registerFuture(call.callId, Promise.resolve(returned))
      return { case: 'future', value: call.callId }
    }
    return sendableResult(returned)
  }

  /** Tracks a promise so `ResolveFutures` can later deliver its outcome. */
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
  private async handleResolveFutures(event: ResolveFutures): Promise<Request['kind']> {
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
    const results = pending
      .filter((f) => f.done)
      .map((f) => {
        this.futures.delete(f.callId)
        const outcome = f.outcome!
        const kind: ExtResult['kind'] =
          'ok' in outcome ? sendableResult(outcome.ok) : { case: 'error', value: jsErrorToProto(outcome.err) }
        return create(FutureResultSchema, { callId: f.callId, result: create(ExtResultSchema, { kind }) })
      })
    return { case: 'resumeFutures', value: create(ResumeFuturesSchema, { results }) }
  }

  /** Dispatches an OS call to the `os` callback (or its default error). */
  private async handleOsCall(call: OsCall, os: OsCallback | undefined): Promise<Request['kind']> {
    const notHandled = (): ExtResult['kind'] => ({
      case: 'error',
      value: call.notHandledError ?? pbError('RuntimeError', `unhandled OS call: ${call.functionName}`),
    })
    let result: ExtResult['kind']
    if (os === undefined) {
      result = notHandled()
    } else {
      try {
        let returned: unknown = os(call.functionName, call.args.map(montyToJs), kwargsToObject(call))
        if (isThenable(returned)) {
          returned = await returned
        }
        result = returned === NOT_HANDLED ? notHandled() : sendableResult(returned)
      } catch (err) {
        result = { case: 'error', value: jsErrorToProto(err) }
      }
    }
    return {
      case: 'resumeCall',
      value: create(ResumeCallSchema, { callId: call.callId, result: create(ExtResultSchema, { kind: result }) }),
    }
  }

  /**
   * Runs one protocol turn: sends the request, forwards streamed `Print`
   * events, and returns the turn-ending event. The watchdog kills the worker
   * if the turn outlives its deadline — the tighter of `requestTimeout` and,
   * for execution turns, the remaining `maxDurationSecs` budget plus the
   * grace (the host backstop for a sandbox limit that cannot fire, e.g. a
   * blocking syscall inside a mount). Worker death is converted to
   * [`MontyCrashedError`] and poisons the session.
   */
  private async turn(kind: Request['kind'], printTarget: PrintTarget | null): Promise<Event> {
    // No ensureUsable here: the public entry points check, and close() runs
    // its Reset turn after the session is already flagged as closed.
    // Execution turns are the ones where the sandbox runs code; control
    // turns (replCreate, dump, reset) have no sandbox budget.
    const execution =
      kind.case === 'replFeed' ||
      kind.case === 'resumeCall' ||
      kind.case === 'resumeNameLookup' ||
      kind.case === 'resumeFutures'
    let deadlineMs = this.requestTimeoutMs
    if (execution && this.durationBudgetMs !== null && this.durationGraceMs !== null) {
      const backstop = Math.max(0, this.durationBudgetMs - this.reportedExecutionMs) + this.durationGraceMs
      deadlineMs = deadlineMs === null ? backstop : Math.min(deadlineMs, backstop)
    }
    let watchdog: NodeJS.Timeout | null = null
    if (deadlineMs !== null) {
      watchdog = setTimeout(() => {
        this.worker.killedForTimeout = true
        this.worker.kill()
      }, deadlineMs)
    }
    try {
      await this.worker.send(kind)
      for (;;) {
        const event = await this.worker.readEvent()
        if (event.kind.case === 'print') {
          printTarget?.write(event.kind.value.stream === 2 ? 'stderr' : 'stdout', event.kind.value.text)
          continue
        }
        // Turn-ending events are stamped with the worker's cumulative
        // execution time; adopt it (ratcheting up only) for the backstop.
        this.reportedExecutionMs = Math.max(this.reportedExecutionMs, Number(event.totalExecutionMicros) / 1000)
        if (event.kind.case === 'fatalError') {
          throw new ProtocolError(`worker reported a fatal error: ${event.kind.value.message}`)
        }
        return event
      }
    } catch (err) {
      // Crash, timeout kill, or protocol desync: the worker is unusable.
      this.broken = err as Error
      throw err
    } finally {
      if (watchdog !== null) {
        clearTimeout(watchdog)
      }
    }
  }

  /** Poisons the session over an event no request expects. */
  private unexpected(event: Event, request: string): ProtocolError {
    const err = new ProtocolError(`unexpected ${event.kind.case ?? 'empty'} event in response to ${request}`)
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
      // Captured and re-thrown at the turn boundary: a throwing callback
      // must not desync the wire protocol mid-turn.
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
function buildCallArgs(call: FunctionCall): unknown[] {
  const args = call.args.map(montyToJs)
  if (call.kwargs.length > 0) {
    args.push(kwargsToObject(call))
  }
  return args
}

/** Converts wire kwargs pairs to a plain object (keys are always strings). */
function kwargsToObject(call: FunctionCall | OsCall): Record<string, unknown> {
  const kwargs: Record<string, unknown> = {}
  for (const p of call.kwargs) {
    if (p.key?.kind.case === 'str' && p.value !== undefined) {
      kwargs[p.key.kind.value] = montyToJs(p.value)
    }
  }
  return kwargs
}

/** Converts a feed input, enforcing the wire depth bound before sending. */
function convertInput(js: unknown) {
  const value = jsToMonty(js)
  if (exceedsMaxValueDepth(value)) {
    throw new MontyError('RuntimeError', 'Max input depth exceeded')
  }
  return value
}

/**
 * Converts an external call's return value. Values that cannot cross the
 * wire — unconvertible, malformed markers, or too deeply nested — become an
 * in-sandbox error instead: catchable, and the session survives. This
 * function must never throw, because its caller has a suspended worker
 * awaiting the resume this result goes into.
 */
function sendableResult(returned: unknown): ExtResult['kind'] {
  let value
  try {
    value = jsToMonty(returned)
  } catch (err) {
    const excType = err instanceof ConversionError ? 'TypeError' : 'RuntimeError'
    return { case: 'error', value: pbError(excType, err instanceof Error ? err.message : String(err)) }
  }
  if (exceedsMaxValueDepth(value)) {
    return { case: 'error', value: pbError('RuntimeError', 'Max input depth exceeded') }
  }
  return { case: 'returnValue', value }
}

/**
 * Maps a thrown JS value to a wire exception the sandbox re-raises. The JS
 * error's `name` is used when it matches a Python exception type (Python
 * code can catch `TypeError` from a JS `TypeError`); anything else becomes
 * `RuntimeError`.
 */
function jsErrorToProto(err: unknown): PbMontyError {
  if (err instanceof MontyError) {
    const { typeName, message } = err.exception
    return pbError(typeName, message)
  }
  if (err instanceof Error) {
    const excType = PYTHON_EXC_NAMES.has(err.name) ? err.name : 'RuntimeError'
    return pbError(excType, err.message)
  }
  return pbError('RuntimeError', String(err))
}

function pbError(excType: string, message: string): PbMontyError {
  return create(MontyErrorSchema, { excType, message })
}

function isThenable(value: unknown): value is PromiseLike<unknown> {
  return typeof value === 'object' && value !== null && typeof (value as { then?: unknown }).then === 'function'
}

function missingField(field: string): never {
  throw new ProtocolError(`missing ${field} in event from worker`)
}
