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

import type { NativeSession } from '../native-addon.js'
import {
  MontyCrashedError,
  MontyError,
  montyErrorFromNative,
  MontyTypingError,
  notCallableMessage,
  ProtocolError,
} from './errors.js'
import { PYTHON_EXC_NAMES } from './errors.js'
import { mountsToNative, type MountDir } from './mount.js'
import type {
  FunctionCallTurn,
  LoadedTurn,
  NameLookupTurn,
  NativeFutureResult,
  NativeTurn,
  OkTurn,
  NotMountedTurn,
  OsCallTurn,
  ResolveFuturesTurn,
} from './native.js'
import { CollectString, CollectStreams } from './print.js'

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

/**
 * Values accepted by `printCallback` options (function or host collectors).
 * Mirrors pydantic_monty's PrintCallback union.
 */
export type PrintTargetInput = PrintCallback | CollectString | CollectStreams

/** Options for [`MontySession.feedRun`]. */
export interface FeedOptions {
  /** Values bound as globals before the snippet runs. */
  inputs?: Record<string, unknown>
  /**
   * Host values the sandbox may reference by an otherwise-undefined name,
   * resolved lazily on demand: a function entry becomes a host function the
   * sandbox can call (sync or async), any other value is converted and
   * returned directly when the name is read, and an absent name raises
   * `NameError`. The lazy counterpart to `inputs`, which eagerly binds every
   * entry as a global whether or not it is referenced; a name in both is
   * served by the eager `inputs` binding.
   */
  externalLookup?: Record<string, unknown>
  /** Receives `print()` output; defaults to the host process stdout/stderr. */
  printCallback?: PrintTargetInput
  /** Host directories mounted into the sandbox for this feed. */
  mount?: MountDir | MountDir[]
  /** Handler for OS calls not covered by mounts. */
  os?: OsCallback
  /** Skip type checking for this feed even when the session enables it. */
  skipTypeCheck?: boolean
}

/**
 * Options for [`MontySession.feedStart`]. Like [`FeedOptions`], but
 * `externalLookup` (and `os`) are *not* dispatched during the initial drive —
 * external calls and name lookups are surfaced as snapshots. They are captured
 * so [`snapshot.resumeAuto()`](FunctionSnapshot#resumeAuto) can answer each
 * suspension from them, letting a caller iterate to completion by hand.
 */
export interface FeedStartOptions {
  /** Values bound as globals before the snippet runs. */
  inputs?: Record<string, unknown>
  /**
   * Host functions and values the sandbox may reference by name, as in
   * [`FeedOptions.externalLookup`]. Captured for `resumeAuto()`; not consulted
   * by a plain `snapshot.resume(...)`.
   */
  externalLookup?: Record<string, unknown>
  /** Receives `print()` output; defaults to the host process stdout/stderr. */
  printCallback?: PrintTargetInput
  /** Host directories mounted into the sandbox for this feed. */
  mount?: MountDir | MountDir[]
  /** Handler for OS calls not covered by mounts. Consulted only by
   *  `resumeAuto()` — `feedStart` always surfaces OS calls as snapshots. */
  os?: OsCallback
  /** Skip type checking for this feed even when the session enables it. */
  skipTypeCheck?: boolean
}

/** Options for [`MontySession.loadSnapshot`]. */
export interface LoadSnapshotOptions {
  /** Receives `print()` output from the resumed feed. */
  printCallback?: PrintTargetInput
  /** The mounts the paused feed used (re-established by value; host paths are
   *  not stored in the dump). Validated against the dump's requirements. */
  mount?: MountDir | MountDir[]
  /**
   * Host functions and values captured for `resumeAuto()` on the restored
   * snapshot, as in [`FeedStartOptions.externalLookup`]. A restored
   * `FutureSnapshot` cannot be `resumeAuto()`'d — its pending promises lived in
   * the previous process; resolve it manually with `resume([...])`.
   */
  externalLookup?: Record<string, unknown>
  /** Handler for OS calls, consulted by `resumeAuto()` as in `feedStart`. */
  os?: OsCallback
}

/** What [`MontySession.feedStart`] / `resume` / `loadSnapshot` yield. */
export type Snapshot = FunctionSnapshot | NameLookupSnapshot | FutureSnapshot | MontyComplete

/** A settled future outcome passed to [`FutureSnapshot.resume`]. */
export type FutureResolution = { callId: number; value: unknown } | { callId: number; error: unknown }

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
  /** Set once the session has been fed or restored; `loadSession` /
   *  `loadSnapshot` are valid only while unset (a fresh session). */
  private driven = false

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
    this.driven = true
    const printTarget = new PrintTarget(options.printCallback)
    const onPrint = printTarget.write.bind(printTarget)
    // A fresh answerer (and its pending-future map) per feed, so promises the
    // worker never asks about again cannot accumulate across feeds.
    const answerer = new TurnAnswerer(this.native, options.externalLookup, options.os)
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
      // Print failures take precedence. On a suspension the worker is waiting
      // for a resume we will never send — poison so the next feed fails cleanly
      // (matches Python pool: discard checkout when print fails mid-suspend).
      try {
        printTarget.throwIfFailed()
      } catch (err) {
        throw this.poison(err instanceof Error ? err : new Error(String(err)))
      }
      try {
        turn = await answerer.answer(turn, onPrint)
      } catch (err) {
        // A handler that throws instead of answering leaves the worker
        // suspended, awaiting a resume that will never come — the session
        // cannot be trusted any more.
        this.broken ??= err instanceof Error ? err : new Error(String(err))
        throw err
      }
    }
  }

  /**
   * Starts a snippet but, instead of driving it to completion, returns a
   * snapshot at each external call, OS call, name lookup, or future
   * resolution. Answer it with `snapshot.resume(...)`, which resolves to the
   * next snapshot or a `MontyComplete`.
   *
   * Alternatively, pass an `externalLookup` (and/or `os`) and drive the whole
   * snippet with `snapshot.resumeAuto()`, which answers each suspension from
   * them automatically:
   *
   * ```ts
   * let snap = await session.feedStart(code, { externalLookup: { fetch } })
   * while (!(snap instanceof MontyComplete)) snap = await snap.resumeAuto()
   * ```
   *
   * Unlike `feedRun`, nothing is answered during this drive: external calls,
   * name lookups and OS calls are all surfaced as snapshots, so even a
   * mount-covered file read comes back as one. `externalLookup` / `os` are
   * captured for later `resumeAuto()` calls, which also consult the feed's
   * mounts. Use `snapshot.dump()` to checkpoint the worker and `loadSnapshot`
   * to restore it.
   */
  async feedStart(code: string, options: FeedStartOptions = {}): Promise<Snapshot> {
    this.ensureUsable()
    this.driven = true
    const driver = this.newDriver(options)
    const turn = (await this.native.feed(
      code,
      options.inputs ?? null,
      mountsToNative(options.mount),
      options.skipTypeCheck ?? false,
      driver.onPrint,
    )) as NativeTurn
    return driver.advance(turn)
  }

  /**
   * Restores a dumped **idle** session — bytes from `session.dump()` taken
   * between feeds — so you can keep feeding it. Use [`loadSnapshot`] for a dump
   * taken mid-execution.
   *
   * Valid only on a fresh session, before any feed or load (it replaces the
   * whole session); throws otherwise. The dump restores its own resource limits
   * and type-check state. Throws if the dump is actually a suspended snapshot.
   */
  async loadSession(state: Uint8Array): Promise<void> {
    this.claimFresh()
    const printTarget = new PrintTarget(undefined)
    const turn = (await this.native.restore(bytesForNative(state), [], printTarget.write.bind(printTarget))) as
      | NativeTurn
      | LoadedTurn
    switch (turn.kind) {
      case 'loaded':
        return
      case 'crashed':
        throw await this.failedLoad(new MontyCrashedError(turn.message, turn))
      case 'protocol':
        throw await this.failedLoad(new ProtocolError(turn.message))
      case 'error':
        throw await this.failedLoad(montyErrorFromNative(turn.exception))
      default:
        throw await this.failedLoad(new Error('this dump is a suspended snapshot — use loadSnapshot() to resume it'))
    }
  }

  /**
   * Restores a dumped **suspended** snapshot — bytes from `feedStart` +
   * `snapshot.dump()` — and resolves to the snapshot to resume. Use
   * [`loadSession`] for a dump taken between feeds.
   *
   * Valid only on a fresh session, before any feed or load; throws otherwise.
   * Re-supply the same `mount`s the paused feed used (their host paths are not
   * in the dump), or its filesystem calls degrade into unhandled OS calls.
   * Throws if the dump is actually an idle session.
   */
  async loadSnapshot(state: Uint8Array, options: LoadSnapshotOptions = {}): Promise<Snapshot> {
    this.claimFresh()
    const driver = this.newDriver(options)
    const turn = (await this.native.restore(bytesForNative(state), mountsToNative(options.mount), driver.onPrint)) as
      | NativeTurn
      | LoadedTurn
    if (turn.kind === 'loaded') {
      throw await this.failedLoad(new Error('this dump is an idle session — use loadSession() to restore it'))
    }
    try {
      return await driver.advance(turn)
    } catch (err) {
      // any failure restoring the snapshot (bad mount, crash, protocol desync)
      // leaves the session unusable — poison it and release the worker
      throw await this.failedLoad(err instanceof Error ? err : new Error(String(err)))
    }
  }

  /** Claims a fresh session for a load (rejecting a reused one). */
  private claimFresh(): void {
    this.ensureUsable()
    if (this.driven) {
      throw new Error(
        'loadSession / loadSnapshot is only valid on a fresh session, before any feedRun / feedStart / loadSession / loadSnapshot',
      )
    }
    this.driven = true
  }

  /**
   * Poisons the session and releases its worker after a failed load, so any
   * later op fails like a crashed session — a failed load is not retryable.
   * Returns the error to throw.
   */
  private async failedLoad(err: Error): Promise<Error> {
    this.poison(err)
    try {
      await this.native.finish()
    } catch {
      // the worker was already discarded (e.g. it crashed) — nothing to release
    }
    return err
  }

  /** Builds the per-feed snapshot driver (print target, answerer, poison). The
   *  captured `externalLookup` / `os` back `snapshot.resumeAuto()`. */
  private newDriver(options: FeedStartOptions): SnapshotDriver {
    const printTarget = new PrintTarget(options.printCallback)
    const answerer = new TurnAnswerer(this.native, options.externalLookup, options.os)
    return new SnapshotDriver(this.native, printTarget, answerer, (err) => this.poison(err))
  }

  /**
   * Serializes the worker's session state into opaque bytes via monty's dump
   * format. The session stays usable; the bytes can only be restored by a
   * monty worker of the same version.
   */
  async dump(): Promise<Buffer> {
    this.ensureUsable()
    return bufferFrom(await this.native.dump())
  }

  /**
   * Installs third-party Python packages into the session via the worker's
   * `uv`, making them importable by later `feedRun` calls. Session-scoped and
   * repeatable; an empty list is a no-op.
   *
   * Only supported by an embedded-CPython worker.
   * Against the pure-Monty sandbox worker, or on a `uv` install failure (the
   * error carries uv's stderr), throws `MontyRuntimeError`; the session stays
   * usable. Dependencies a script declares inline via PEP 723 (`# /// script`)
   * are installed automatically on `feedRun` and need no call here.
   */
  async installDependencies(requirements: string[]): Promise<void> {
    this.ensureUsable()
    // mark the session driven so a later loadSession/loadSnapshot is rejected —
    // it would discard the freshly installed environment
    this.driven = true
    const printTarget = new PrintTarget(undefined)
    const turn = (await this.native.installDependencies(requirements, printTarget.write.bind(printTarget))) as
      | NativeTurn
      | OkTurn
    switch (turn.kind) {
      case 'ok':
        return
      case 'error':
        throw montyErrorFromNative(turn.exception)
      case 'crashed':
        throw this.poison(new MontyCrashedError(turn.message, turn))
      case 'protocol':
        throw this.poison(new ProtocolError(turn.message))
      default:
        throw this.poison(new ProtocolError(`unexpected turn kind: ${(turn as { kind: string }).kind}`))
    }
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

/**
 * Answers one suspension turn from a captured `externalLookup` / `os`, tracking
 * promise-returning externals as pending futures. Shared by
 * [`MontySession.feedRun`]'s drive loop and [`SnapshotDriver`]'s `resumeAuto`
 * so both resolve suspensions identically. Built fresh per feed / per snapshot
 * chain — its `futures` map is scoped to that run, never leaking across feeds.
 *
 * `answer` deliberately does **not** catch: a handler that throws leaves the
 * worker suspended, so the caller poisons the session and rethrows.
 */
class TurnAnswerer {
  /** Pending async external calls, by call id. */
  readonly futures = new Map<number, PendingFuture>()

  constructor(
    private readonly native: NativeSession,
    readonly externalLookup: Record<string, unknown> | undefined,
    readonly os: OsCallback | undefined,
  ) {}

  /** Answers one suspension turn and returns the resume turn it produces. */
  async answer(
    turn: FunctionCallTurn | OsCallTurn | ResolveFuturesTurn | NameLookupTurn,
    onPrint: PrintCallback,
  ): Promise<NativeTurn> {
    let next: Promise<object>
    switch (turn.kind) {
      case 'functionCall':
        next = this.answerFunctionCall(turn, onPrint)
        break
      case 'osCall':
        next = this.answerOsCall(turn, onPrint)
        break
      case 'nameLookup': {
        // A callable entry resolves to a host function (by display name); any
        // other value is converted and returned directly; an absent name is
        // left undefined so the sandbox raises NameError. Only own keys count
        // — an inherited member (`toString`, `constructor`, …) must never
        // satisfy a lookup the host did not deliberately expose. The value is
        // wrapped in `{ value }` so `null`/`undefined` entries resolve to
        // `None` instead of reading as "no value" through napi.
        const lookup = this.externalLookup
        if (lookup === undefined || !Object.prototype.hasOwnProperty.call(lookup, turn.name)) {
          next = this.native.resumeNameLookup(null, null, onPrint)
        } else {
          const v = lookup[turn.name]
          if (typeof v === 'function') {
            next = this.native.resumeNameLookup((v as ExternalFunction).name || '<anonymous>', null, onPrint)
          } else {
            next = this.native.resumeNameLookup(null, { value: v }, onPrint)
          }
        }
        break
      }
      case 'resolveFutures':
        next = this.answerResolveFutures(turn, onPrint)
        break
    }
    return (await next) as NativeTurn
  }

  /** Calls the matching external function and resumes with its result. */
  private answerFunctionCall(call: FunctionCallTurn, onPrint: PrintCallback): Promise<object> {
    if (call.methodCall) {
      // Dataclass method dispatch needs host-side class objects, which this
      // package has no registry for (unlike pydantic_monty).
      return this.native.resumeError(
        'RuntimeError',
        `method calls on host objects are not supported: ${call.functionName}`,
        onPrint,
      )
    }
    // Own keys only, as in the nameLookup branch: an inherited callable (e.g.
    // `Object.prototype.toString`) must never be dispatched as a host function.
    const externalLookup = this.externalLookup
    if (externalLookup === undefined || !Object.prototype.hasOwnProperty.call(externalLookup, call.functionName)) {
      return this.native.resumeNotFound(onPrint)
    }
    const entry = externalLookup[call.functionName]
    if (typeof entry !== 'function') {
      // A cached function proxy whose entry was later replaced by a plain
      // value: raise what CPython would for calling that value, matching the
      // Python binding (which really calls the entry).
      return this.native.resumeError('TypeError', notCallableMessage(entry), onPrint)
    }
    const fn = entry as ExternalFunction
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

  /**
   * Answers an OS call: the feed's mounts get first refusal, then the `os`
   * callback, then the sandbox's own no-handler default.
   */
  async answerOsCall(call: OsCallTurn, onPrint: PrintCallback): Promise<object> {
    const mounted = (await this.native.resumeFromMounts(onPrint)) as NativeTurn | NotMountedTurn
    if (mounted.kind !== 'notMounted') {
      return mounted
    }
    if (this.os === undefined) {
      return await this.native.resumeNotHandled(onPrint)
    }
    let returned: unknown
    try {
      returned = this.os(call.functionName, call.args, kwargsToRecord(call.kwargs))
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
}

/** A new `PrintTarget` per feed: routes prints, capturing callback failures. */
class PrintTarget {
  private readonly callback: PrintCallback | undefined
  private failure: unknown = null

  constructor(input: PrintTargetInput | undefined) {
    // Collectors adapt once to a function so the drive loop / native path stay typed as PrintCallback.
    if (input instanceof CollectString || input instanceof CollectStreams) {
      this.callback = (stream, text) => input.write(stream, text)
    } else {
      this.callback = input
    }
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

/**
 * Drives a `feedStart` / `loadSnapshot` chain: runs each protocol turn and
 * turns the result into the next [`Snapshot`], answering nothing itself.
 * One driver is shared across a snapshot and every snapshot its `resume`
 * produces, so they all answer the same worker with the same print sink.
 */
class SnapshotDriver {
  /** Exposed so the session's first turn can stream prints through it. */
  readonly onPrint: PrintCallback

  constructor(
    private readonly native: NativeSession,
    private readonly printTarget: PrintTarget,
    private readonly answerer: TurnAnswerer,
    private readonly poison: (err: Error) => Error,
  ) {
    this.onPrint = printTarget.write.bind(printTarget)
  }

  /** Resolves a turn to the next snapshot, answering nothing automatically. */
  async advance(turn: NativeTurn): Promise<Snapshot> {
    for (;;) {
      switch (turn.kind) {
        case 'complete':
          this.printTarget.throwIfFailed()
          return new MontyComplete(turn.value)
        case 'error':
          this.printTarget.throwIfFailed()
          throw montyErrorFromNative(turn.exception)
        case 'typingError':
          this.printTarget.throwIfFailed()
          throw new MontyTypingError(turn.diagnostics)
        case 'crashed':
          throw this.poison(new MontyCrashedError(turn.message, turn))
        case 'protocol':
          throw this.poison(new ProtocolError(turn.message))
        case 'osCall':
          // Every OS call surfaces; mounts and the `os` callback are consulted
          // only by `resumeAuto`.
          this.throwIfPrintFailedOnSuspend()
          return new FunctionSnapshot(this, turn, true)
        case 'functionCall':
          this.throwIfPrintFailedOnSuspend()
          return new FunctionSnapshot(this, turn, false)
        case 'nameLookup':
          this.throwIfPrintFailedOnSuspend()
          return new NameLookupSnapshot(this, turn)
        case 'resolveFutures':
          this.throwIfPrintFailedOnSuspend()
          return new FutureSnapshot(this, turn)
        default:
          throw this.poison(new ProtocolError(`unexpected turn kind: ${(turn as { kind: string }).kind}`))
      }
    }
  }

  /**
   * Surfaces a pending print-callback failure before exposing a suspension
   * snapshot. The worker is left suspended with no resume coming, so poison
   * the session (same policy as Python's checkout discard on this path).
   */
  private throwIfPrintFailedOnSuspend(): void {
    try {
      this.printTarget.throwIfFailed()
    } catch (err) {
      throw this.poison(err instanceof Error ? err : new Error(String(err)))
    }
  }

  /**
   * Auto-answers a suspension `turn` from the captured `externalLookup` / `os`
   * (via the shared [`TurnAnswerer`], identically to `feedRun`), then advances
   * to the next snapshot. A handler that throws poisons the session.
   */
  async resumeAuto(turn: FunctionCallTurn | OsCallTurn | NameLookupTurn | ResolveFuturesTurn): Promise<Snapshot> {
    let next: NativeTurn
    try {
      next = await this.answerer.answer(turn, this.onPrint)
    } catch (err) {
      throw this.poison(err instanceof Error ? err : new Error(String(err)))
    }
    return this.advance(next)
  }

  // resume primitives — each runs one turn, then advances to the next snapshot

  async resumeReturn(value: unknown): Promise<Snapshot> {
    return this.advance((await this.native.resumeReturn(value, this.onPrint)) as NativeTurn)
  }

  async resumeError(err: unknown): Promise<Snapshot> {
    const { excType, message } = jsErrorParts(err)
    return this.advance((await this.native.resumeError(excType, message, this.onPrint)) as NativeTurn)
  }

  async resumeNotFound(): Promise<Snapshot> {
    return this.advance((await this.native.resumeNotFound(this.onPrint)) as NativeTurn)
  }

  async resumeNotHandled(): Promise<Snapshot> {
    return this.advance((await this.native.resumeNotHandled(this.onPrint)) as NativeTurn)
  }

  async resumeFuture(): Promise<Snapshot> {
    return this.advance((await this.native.resumeFuture(this.onPrint)) as NativeTurn)
  }

  async resumeNameLookup(functionName: string | null): Promise<Snapshot> {
    // feedStart name-lookup snapshots only resolve to functions (by name).
    return this.advance((await this.native.resumeNameLookup(functionName, null, this.onPrint)) as NativeTurn)
  }

  async resolveFutures(results: NativeFutureResult[]): Promise<Snapshot> {
    return this.advance((await this.native.resolveFutures(results, this.onPrint)) as NativeTurn)
  }

  async dump(): Promise<Buffer> {
    return bufferFrom(await this.native.dump())
  }
}

/** Marks a snapshot single-use: each may be resumed at most once. */
class SingleUse {
  private used = false
  protected claim(): void {
    if (this.used) {
      throw new Error('snapshot has already been resumed')
    }
    this.used = true
  }
}

/**
 * A paused execution waiting for an external function or OS call result. For
 * OS calls `isOsFunction` is `true`; resume with a value, an error, or
 * `resumeNotHandled()`.
 */
export class FunctionSnapshot extends SingleUse {
  readonly functionName: string
  /** Positional arguments, already converted to JS values. */
  readonly args: unknown[]
  /** Keyword arguments (null-prototype record; string keys only). */
  readonly kwargs: Record<string, unknown>
  readonly callId: number
  readonly isOsFunction: boolean
  readonly isMethodCall: boolean

  /** @internal */
  constructor(
    private readonly driver: SnapshotDriver,
    private readonly turn: FunctionCallTurn | OsCallTurn,
    isOsFunction: boolean,
  ) {
    super()
    this.functionName = turn.functionName
    this.args = turn.args
    this.kwargs = kwargsToRecord(turn.kwargs)
    this.callId = turn.callId
    this.isOsFunction = isOsFunction
    this.isMethodCall = 'methodCall' in turn ? turn.methodCall : false
  }

  /** Resumes with the call's return value. */
  resume(value: unknown): Promise<Snapshot> {
    this.claim()
    return this.driver.resumeReturn(value)
  }

  /**
   * Answers this call automatically from the `externalLookup` / `os` captured
   * at `feedStart` / `loadSnapshot`, then resolves to the next snapshot (or
   * `MontyComplete`). A name absent from `externalLookup` makes the sandbox
   * raise `NameError`; a promise-returning external is registered as a future
   * (settled later by [`FutureSnapshot.resumeAuto`]). Resumes at most once.
   */
  resumeAuto(): Promise<Snapshot> {
    this.claim()
    return this.driver.resumeAuto(this.turn)
  }

  /** Resumes by raising the given error inside the sandbox. */
  resumeError(error: unknown): Promise<Snapshot> {
    this.claim()
    return this.driver.resumeError(error)
  }

  /** Resumes as "no such function": the sandbox raises `NameError`. */
  resumeNotFound(): Promise<Snapshot> {
    this.claim()
    return this.driver.resumeNotFound()
  }

  /** Registers the call as a pending future; other sandbox tasks keep
   *  running and surface later as a [`FutureSnapshot`]. */
  resumeFuture(): Promise<Snapshot> {
    this.claim()
    return this.driver.resumeFuture()
  }

  /** Resumes an OS call with monty's default unhandled behaviour. */
  resumeNotHandled(): Promise<Snapshot> {
    if (!this.isOsFunction) {
      throw new Error('resumeNotHandled is only valid for OS-call snapshots')
    }
    this.claim()
    return this.driver.resumeNotHandled()
  }

  /** Serializes the paused worker; restore with `session.loadSnapshot`. */
  dump(): Promise<Buffer> {
    return this.driver.dump()
  }
}

/** A paused execution waiting for the value of an undefined name. */
export class NameLookupSnapshot extends SingleUse {
  readonly variableName: string

  /** @internal */
  constructor(
    private readonly driver: SnapshotDriver,
    private readonly turn: NameLookupTurn,
  ) {
    super()
    this.variableName = turn.name
  }

  /** Resolves the name to an external function by name, or — with no argument
   *  — lets the sandbox raise `NameError`. */
  resume(functionName?: string): Promise<Snapshot> {
    this.claim()
    return this.driver.resumeNameLookup(functionName ?? null)
  }

  /**
   * Answers this name lookup automatically from the captured `externalLookup`,
   * then resolves to the next snapshot. Unlike `resume` (function-by-name only),
   * this resolves to any captured value or function; an absent name makes the
   * sandbox raise `NameError`. Resumes at most once.
   */
  resumeAuto(): Promise<Snapshot> {
    this.claim()
    return this.driver.resumeAuto(this.turn)
  }

  dump(): Promise<Buffer> {
    return this.driver.dump()
  }
}

/** A paused execution where every sandbox task is blocked on external futures. */
export class FutureSnapshot extends SingleUse {
  readonly pendingCallIds: number[]

  /** @internal */
  constructor(
    private readonly driver: SnapshotDriver,
    private readonly turn: ResolveFuturesTurn,
  ) {
    super()
    this.pendingCallIds = turn.pendingCallIds
  }

  /** Delivers settled outcomes for one or more pending futures, by call id. */
  resume(results: FutureResolution[]): Promise<Snapshot> {
    this.claim()
    const native: NativeFutureResult[] = results.map((result) => {
      if ('error' in result) {
        const { excType, message } = jsErrorParts(result.error)
        return { callId: result.callId, ok: false, excType, message }
      }
      return { callId: result.callId, ok: true, value: result.value }
    })
    return this.driver.resolveFutures(native)
  }

  /**
   * Waits for one or more of the pending promises registered by earlier
   * `resumeAuto` calls to settle, delivers them, and resolves to the next
   * snapshot. Throws if there are no tracked promises for these ids — e.g. on a
   * snapshot restored via `loadSnapshot`, whose promises lived in the previous
   * process (resolve those manually with `resume([...])`). Resumes at most once.
   */
  resumeAuto(): Promise<Snapshot> {
    this.claim()
    return this.driver.resumeAuto(this.turn)
  }

  dump(): Promise<Buffer> {
    return this.driver.dump()
  }
}

/** The result of a completed `feedStart` execution. */
export class MontyComplete {
  /** @internal */
  constructor(readonly output: unknown) {}
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

function bytesForNative(bytes: Uint8Array): Buffer {
  return (typeof Buffer === 'undefined' ? bytes : Buffer.from(bytes)) as Buffer
}

function bufferFrom(bytes: Uint8Array): Buffer {
  return (typeof Buffer === 'undefined' ? bytes : Buffer.from(bytes)) as Buffer
}
