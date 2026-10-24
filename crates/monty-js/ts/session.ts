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

import { context as ContextAPI, type Context } from '@opentelemetry/api'

import type { NativeSession } from '../native-addon.js'
import { bindPrintCallback, getCallbackContext, runWithCallbackContext } from './callbackContext.js'
import {
  AttrNotExposed,
  attributeErrorMessage,
  HostModuleValue,
  InstanceStore,
  prepare,
  restore,
  type WalkMemo,
} from './classInstance.js'
import {
  MontyCrashedError,
  MontyDisconnectError,
  MontyError,
  montyErrorFromNative,
  MontyShutdown,
  MontyTypingError,
  notCallableMessage,
  ProtocolError,
  type SourceRange,
} from './errors.js'
import { PYTHON_EXC_NAMES } from './errors.js'
import { checkModuleName, ExternalModule, kindOf } from './externalModule.js'
import { mountsToNative } from './mount.js'
import type { MountDir } from './mountDir.js'
import {
  type CrashedTurn,
  type DisconnectedTurn,
  type DumpedTurn,
  type EnterTurn,
  type FunctionCallTurn,
  type LoadedTurn,
  type NameLookupTurn,
  type NativeFutureResult,
  type NativeTurn,
  type OkTurn,
  type NotMountedTurn,
  type OsCallTurn,
  type ResolveFuturesTurn,
  type ShutdownTurn,
  osCallAcceptsFuture,
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

/** Receives sandbox `print()` output in batched chunks. */
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
  /**
   * Switches the sandbox's working directory before this feed, an absolute
   * virtual path. The directory persists across the session's feeds,
   * including any `os.chdir()`, so leaving it unset keeps the current one;
   * the session's first feed defaults to its first mount's virtual path, or
   * `/` without mounts. `os.getcwd()` reports it and relative paths resolve
   * against it before reaching a mount or the `os` handler.
   */
  cwd?: string
  /** Handler for OS calls not covered by mounts. */
  os?: OsCallback
  /** Skip type checking for this feed even when the session enables it. */
  skipTypeCheck?: boolean
}

/**
 * Options for [`MontySession.feedStart`]. External calls, imports and lookups return snapshots.
 * [`snapshot.resumeAuto()`](FunctionSnapshot#resumeAuto) uses the captured `externalLookup`
 * and `os`, and the session's `externalModules`, to answer each suspension.
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
  /** Switches the sandbox's working directory before the feed, as in
   *  [`FeedOptions.cwd`]; a dump taken mid-feed carries it. */
  cwd?: string
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
 * One worker dedicated to one REPL session; created by
 * [`Monty.checkout`]. Session state (globals, functions) persists across
 * `feedRun` calls. Close it (or `await using`) to return the worker to the
 * pool.
 */
export class MontySession {
  /** Stable worker identity within its pool, retained during turns and after session close. */
  readonly workerId: number | undefined

  private readonly native: NativeSession
  /** Set once the session is unusable: crashed worker or protocol error. */
  private broken: Error | null = null
  private closed = false
  /** Set once the session has been fed or restored; `loadSession` /
   *  `loadSnapshot` are valid only while unset (a fresh session). */
  private driven = false
  /** Session-scoped `ClassInstance` registry (mirrors pydantic_monty): every
   *  wrapper sent through any boundary of this session lives here so method
   *  calls, lazy lookups and returns route back to the original object. */
  private readonly instances = new InstanceStore()

  /** @internal — sessions are created by `Monty.checkout`, which validates the modules first. */
  constructor(
    native: NativeSession,
    /** The checkout's `externalModules`, answering every feed's imports. */
    private readonly modules: HostModules = new HostModules(undefined),
  ) {
    this.native = native
    this.workerId = native.workerId ?? undefined
  }

  /**
   * Executes one snippet in the worker, driving external function calls
   * (which may return promises), OS callbacks, and print callbacks in this
   * process. Returns the snippet's trailing expression value.
   *
   * Node callbacks preserve the caller's async context. With telemetry enabled,
   * spans created inside callbacks nest under the corresponding Monty operation.
   */
  async feedRun(code: string, options: FeedOptions = {}): Promise<unknown> {
    this.ensureUsable()
    this.driven = true
    const printTarget = new PrintTarget(options.printCallback)
    const onPrint = bindPrintCallback(printTarget.write.bind(printTarget))
    // Discard pending futures between feeds.
    const answerer = new TurnAnswerer(this.native, this.instances, options.externalLookup, this.modules, options.os)
    let turn = (await this.native.feed(
      code,
      prepareInputs(options.inputs, this.instances),
      mountsToNative(options.mount),
      { cwd: options.cwd, skipTypeCheck: options.skipTypeCheck ?? false },
      onPrint,
    )) as NativeTurn
    for (;;) {
      switch (turn.kind) {
        case 'complete':
          printTarget.throwIfFailed()
          return restore(turn.value, this.instances)
        case 'error':
          printTarget.throwIfFailed()
          throw montyErrorFromNative(turn.exception)
        case 'typingError':
          printTarget.throwIfFailed()
          throw new MontyTypingError(turn.diagnostics)
        case 'crashed':
        case 'disconnected':
        case 'shutdown':
          throw this.poison(lostSessionError(turn))
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
      prepareInputs(options.inputs, this.instances),
      mountsToNative(options.mount),
      { cwd: options.cwd, skipTypeCheck: options.skipTypeCheck ?? false },
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
   *
   * Only load unmodified bytes from a trusted, compatible Monty producer.
   * The caller must establish provenance and integrity; Monty does not authenticate
   * snapshots. Invalid snapshots have no correctness or availability guarantees.
   * Successful loading does not establish validity.
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
      case 'disconnected':
      case 'shutdown':
        throw await this.failedLoad(lostSessionError(turn))
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
   * [`loadSession`] for a dump taken between feeds. Its snapshot trust requirements
   * also apply here.
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

  /** Captures the feed's handlers for `snapshot.resumeAuto()`. */
  private newDriver(options: FeedStartOptions | LoadSnapshotOptions): SnapshotDriver {
    const printTarget = new PrintTarget(options.printCallback)
    const answerer = new TurnAnswerer(this.native, this.instances, options.externalLookup, this.modules, options.os)
    return new SnapshotDriver(this.native, this.instances, printTarget, answerer, (err) => this.poison(err))
  }

  /**
   * Serializes the worker's session state into opaque bytes via monty's dump
   * format. The session stays usable; the bytes can only be restored by a
   * monty worker of the same version.
   *
   * Against a `MontyWebsocket` server that stores sessions, returns the ID of
   * a record holding the current state instead; `loadSession` / `loadSnapshot`
   * accept it like dump bytes. A server that stores nothing may refuse with
   * `MontyRuntimeError`, and the session carries on.
   */
  async dump(): Promise<Buffer> {
    this.ensureUsable()
    const printTarget = new PrintTarget(undefined)
    return dumpSession(this.native, printTarget.write.bind(printTarget), (err) => this.poison(err))
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
      case 'disconnected':
      case 'shutdown':
        throw this.poison(lostSessionError(turn))
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
   * The opaque ID a `MontyWebsocket` server that stores sessions gave this
   * session, or `undefined`: local workers, ephemeral sessions and servers that
   * store nothing. Pass it to `loadSession` / `loadSnapshot` on a fresh session
   * to start a new session from the state stored under it. Changes when the
   * pool resumes the session after a server shutdown.
   */
  get sessionId(): Buffer | undefined {
    const id = this.native.sessionId
    return id === undefined || id === null ? undefined : bufferFrom(id)
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
    private readonly instances: InstanceStore,
    readonly externalLookup: Record<string, unknown> | undefined,
    readonly modules: HostModules,
    readonly os: OsCallback | undefined,
  ) {}

  /** Answers one suspension turn and returns the resume turn it produces. */
  answer(
    turn: FunctionCallTurn | OsCallTurn | ResolveFuturesTurn | NameLookupTurn,
    onPrint: PrintCallback,
  ): Promise<NativeTurn> {
    return runWithCallbackContext(turn.callbackSpanKey, () => this.answerInContext(turn, onPrint))
  }

  private async answerInContext(
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
        if (turn.objectId !== undefined && turn.objectId !== null) {
          next = this.answerObjectLookup(turn, turn.objectId, onPrint)
          break
        }
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
            next = this.native.resumeNameLookup(null, { value: prepare(v, this.instances) }, onPrint)
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
  private async answerFunctionCall(call: FunctionCallTurn, onPrint: PrintCallback): Promise<object> {
    if (call.objectId !== undefined && call.objectId !== null) {
      return this.answerMethodCall(call, call.objectId, onPrint)
    }
    let resolved: { value: unknown } | undefined
    try {
      resolved = await this.hostEntry(call.functionName)
    } catch (err) {
      // Raise getter errors at the sandbox call.
      const { excType, message } = jsErrorParts(err)
      return this.native.resumeError(excType, message, onPrint)
    }
    if (resolved === undefined) {
      return this.native.resumeNotFound(onPrint)
    }
    const entry = resolved.value
    if (typeof entry !== 'function') {
      // A cached function proxy whose entry was later replaced by a plain
      // value: raise what CPython would for calling that value, matching the
      // Python binding (which really calls the entry).
      return this.native.resumeError('TypeError', notCallableMessage(entry), onPrint)
    }
    const fn = entry as ExternalFunction
    let returned: unknown
    try {
      const [args, kwargs] = restoreCallArgs(call, this.instances)
      returned = fn(...(buildCallArgs(args, kwargs) as never[]))
    } catch (err) {
      const { excType, message } = jsErrorParts(err)
      return this.native.resumeError(excType, message, onPrint)
    }
    return isThenable(returned)
      ? this.answerAwaitedCall(call, returned, onPrint)
      : this.resumeWithValue(returned, onPrint)
  }

  /**
   * Resolves an own property of `externalLookup` or a dotted module path.
   * Inherited callables must never run. Box the result so this async return
   * cannot invoke a plain value's `then` method.
   */
  private async hostEntry(functionName: string): Promise<{ value: unknown } | undefined> {
    const dot = functionName.lastIndexOf('.')
    if (dot < 0) {
      return boxed(ownEntry(this.externalLookup, functionName))
    }
    const [top, ...rest] = functionName.slice(0, dot).split('.')
    let resolved = await this.module(top as string)
    for (const part of rest) {
      if (resolved === undefined) break
      resolved = submodule(resolved, part)
    }
    if (resolved === undefined) {
      return undefined
    }
    // a module holds no `_` name, so a forged private name finds nothing
    const attr = functionName.slice(dot + 1)
    const entry = ownEntry(resolved.attrs, attr)
    // Preserve the receiver of `module.attr(...)`.
    return boxed(typeof entry === 'function' ? (entry as ExternalFunction).bind(resolved.receiver) : entry)
  }

  /**
   * Resolves a module, caching each factory's result for the session.
   * Await only real Promises. Invalid results raise TypeError at the requesting
   * import, call or stub request (see [`HostModules.install`]).
   */
  private async module(name: string): Promise<ExternalModule | undefined> {
    const resolved = this.modules.resolved.get(name)
    if (resolved !== undefined) {
      return resolved
    }
    const entry = this.modules.entry(name)
    if (entry === undefined) {
      return undefined
    }
    if (entry instanceof ExternalModule) {
      return entry
    }
    const returned = (entry as () => unknown)()
    return this.modules.install(name, returned instanceof Promise ? await returned : returned)
  }

  /**
   * Returns the host module, or not-found for the sandbox to raise `ModuleNotFoundError`.
   */
  private async answerImport(call: OsCallTurn, onPrint: PrintCallback): Promise<object> {
    const [args] = restoreCallArgs(call, this.instances)
    const name = args[0]
    if (typeof name !== 'string') {
      return this.native.resumeNotFound(onPrint)
    }
    let value: unknown
    try {
      // Raise factory and getter errors at the sandbox import.
      const entry = await this.module(name)
      if (entry === undefined) {
        return this.native.resumeNotFound(onPrint)
      }
      value = moduleValue(name, entry)
    } catch (err) {
      const { excType, message } = jsErrorParts(err)
      return this.native.resumeError(excType, message, onPrint)
    }
    return this.resumeWithValue(value, onPrint)
  }

  /**
   * Returns requested modules' `.pyi` stubs and their submodules by dotted path,
   * using `null` for missing stubs. Run factories concurrently and reuse their
   * results for the session's imports.
   */
  private async answerImportStubs(call: OsCallTurn, onPrint: PrintCallback): Promise<object> {
    const [args] = restoreCallArgs(call, this.instances)
    const names = args[0]
    if (!Array.isArray(names) || !names.every((name) => typeof name === 'string')) {
      return this.native.resumeNotFound(onPrint)
    }
    // a null prototype: a module named `__proto__` must land as an own key
    const stubs: Record<string, string | null> = Object.create(null) as Record<string, string | null>
    try {
      const entries = await Promise.all((names as string[]).map((name) => this.module(name)))
      for (const [index, entry] of entries.entries()) {
        if (entry !== undefined) {
          collectStubs(names[index] as string, entry, stubs)
        }
      }
    } catch (err) {
      const { excType, message } = jsErrorParts(err)
      return this.native.resumeError(excType, message, onPrint)
    }
    return this.resumeWithValue(stubs, onPrint)
  }

  /**
   * Dispatches a host-routed call on the object registered under `objectId` —
   * a method on a class instance, or on a `ClassType` wrapper a classmethod
   * or `__call__` construction (the wrapper's own `init` policy gates it).
   * Denied/absent names raise `AttributeError` in the sandbox; a
   * promise-returning method is registered as a pending future.
   */
  private answerMethodCall(call: FunctionCallTurn, objectId: string, onPrint: PrintCallback): Promise<object> {
    const wrapper = this.instances.get(objectId)
    if (call.functionName.startsWith('_') && call.functionName !== '__call__') {
      // Defensive re-check of the sandbox's underscore rule (`__call__` is
      // the one dunder legitimately routed): wire frames from a (possibly
      // compromised) worker are untrusted.
      return this.native.resumeError(
        'AttributeError',
        attributeErrorMessage(wrapper?.getName() ?? 'object', call.functionName),
        onPrint,
      )
    }
    if (wrapper === undefined) {
      // A session restored into a process that never sent the object.
      const message =
        `no host object registered for method call '${call.functionName}' (id ${objectId}) — ` +
        'the instance store is empty after loading a dump into a fresh session'
      return this.native.resumeError('RuntimeError', message, onPrint)
    }
    let returned: unknown
    try {
      const [args, kwargs] = restoreCallArgs(call, this.instances)
      returned = wrapper.callMethod(call.functionName, args, kwargsToRecord(kwargs))
    } catch (err) {
      if (err instanceof AttrNotExposed) {
        return this.native.resumeError('AttributeError', err.message, onPrint)
      }
      const { excType, message } = jsErrorParts(err)
      return this.native.resumeError(excType, message, onPrint)
    }
    return isThenable(returned)
      ? this.answerAwaitedCall(call, returned, onPrint)
      : this.resumeWithValue(returned, onPrint)
  }

  /**
   * Answers a lazy attribute lookup on the host object registered under
   * `objectId` (a class instance, or a class type for lazy class attrs). An
   * undefined answer (`resumeNameLookup(null, null, ...)`) makes the sandbox
   * raise `AttributeError` — used for underscore names, store misses, and
   * names the wrapper's policy denies. Any other host error (a throwing
   * getter, a `convertValue` failure, an unconvertible value) is raised in
   * the sandbox where the lookup suspended, as `answerMethodCall` does for
   * methods; the sandbox only swallows `AttributeError` for `hasattr` and
   * `getattr` defaults, so the exception propagates through those too.
   */
  private answerObjectLookup(turn: NameLookupTurn, objectId: string, onPrint: PrintCallback): Promise<object> {
    const wrapper = this.instances.get(objectId)
    if (wrapper === undefined || turn.name.startsWith('_')) {
      return this.native.resumeNameLookup(null, null, onPrint)
    }
    let outbound: unknown
    try {
      outbound = prepare(wrapper.lookupLazyAttr(turn.name), this.instances)
    } catch (err) {
      if (err instanceof AttrNotExposed) {
        return this.native.resumeNameLookup(null, null, onPrint)
      }
      const { excType, message } = jsErrorParts(err)
      return this.native.resumeNameLookupError(excType, message, onPrint)
    }
    // `resumeLazyAttr`, not `resumeNameLookup`: a native conversion failure
    // must raise in the sandbox too, rather than reject the turn
    return this.native.resumeLazyAttr(outbound, onPrint)
  }

  /**
   * Resumes with a host return value, mapping a `prepare` rejection (e.g. an
   * unwrapped non-plain object) to an in-sandbox error — the same surface a
   * native conversion failure has — instead of poisoning the session.
   */
  private resumeWithValue(value: unknown, onPrint: PrintCallback): Promise<object> {
    let outbound: unknown
    try {
      outbound = prepare(value, this.instances)
    } catch (err) {
      const { excType, message } = jsErrorParts(err)
      return this.native.resumeError(excType, message, onPrint)
    }
    return this.native.resumeReturn(outbound, onPrint)
  }

  /**
   * Handles imports and sleeps locally; other calls try mounts, `os`, then the sandbox default.
   */
  async answerOsCall(call: OsCallTurn, onPrint: PrintCallback): Promise<object> {
    if (call.functionName === IMPORT_CALL) {
      return this.answerImport(call, onPrint)
    }
    if (call.functionName === IMPORT_STUBS_CALL) {
      return this.answerImportStubs(call, onPrint)
    }
    const wait = this.systemSleepFor(call)
    if (wait !== null) {
      if (osCallAcceptsFuture(call.functionName)) {
        return await this.answerAwaitedCall(call, wait, onPrint)
      }
      await wait
      return await this.resumeWithValue(null, onPrint)
    }
    const mounted = (await this.native.resumeFromMounts(onPrint)) as NativeTurn | NotMountedTurn
    if (mounted.kind !== 'notMounted') {
      return mounted
    }
    if (this.os === undefined) {
      return await this.native.resumeNotHandled(onPrint)
    }
    let returned: unknown
    try {
      const [args, kwargs] = restoreCallArgs(call, this.instances)
      returned = this.os(call.functionName, args, kwargsToRecord(kwargs))
      if (isThenable(returned)) {
        if (osCallAcceptsFuture(call.functionName)) {
          return await this.answerAwaitedCall(call, returned, onPrint)
        }
        // The sandbox does not await any other OS call, so a future would be
        // an error: the wait happens here and only this session is held up.
        returned = await returned
      }
    } catch (err) {
      const { excType, message } = jsErrorParts(err)
      return await this.native.resumeError(excType, message, onPrint)
    }
    if (returned === NOT_HANDLED) {
      return await this.native.resumeNotHandled(onPrint)
    }
    return await this.resumeWithValue(returned, onPrint)
  }

  /**
   * Uses the call's sleep marker, so restored sessions need no local copy of their sleep policy.
   */
  private systemSleepFor(call: OsCallTurn): Promise<void> | null {
    return call.systemSleepSecs === undefined ? null : sleepMs(call.systemSleepSecs * 1000)
  }

  /**
   * Answers a call the sandbox awaits with the promise a host callback
   * returned: settled here when the sandbox has nothing else to run, otherwise
   * registered as a future so its other tasks run meanwhile.
   */
  private answerAwaitedCall(
    call: { callId: number; allowEagerAwait?: boolean },
    promise: PromiseLike<unknown>,
    onPrint: PrintCallback,
  ): Promise<object> {
    if (call.allowEagerAwait) {
      return this.answerEagerCoroutine(call.callId, promise, onPrint)
    }
    this.registerFuture(call.callId, Promise.resolve(promise))
    return this.native.resumeFuture(onPrint)
  }

  /** Settles an eligible coroutine at its call suspension, including conversion errors. */
  private async answerEagerCoroutine(
    callId: number,
    promise: PromiseLike<unknown>,
    onPrint: PrintCallback,
  ): Promise<object> {
    let result: NativeFutureResult
    try {
      result = { callId, ok: true, value: prepare(await promise, this.instances) }
    } catch (err) {
      const { excType, message } = jsErrorParts(err)
      result = { callId, ok: false, excType, message }
    }
    return await this.native.resolveFutures([result], onPrint)
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
          // a prepare rejection is delivered as the future's error, matching
          // how a sync return value's conversion failure surfaces
          try {
            return { callId: f.callId, ok: true, value: prepare(outcome.ok, this.instances) }
          } catch (err) {
            const { excType, message } = jsErrorParts(err)
            return { callId: f.callId, ok: false, excType, message }
          }
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
  private failed = false

  constructor(input: PrintTargetInput | undefined) {
    // Collectors adapt once to a function so the drive loop / native path stay typed as PrintCallback.
    if (input instanceof CollectString || input instanceof CollectStreams) {
      this.callback = (stream, text) => input.write(stream, text)
    } else {
      this.callback = input
    }
  }

  write(stream: 'stdout' | 'stderr', text: string): void {
    if (this.failed) {
      return
    }
    if (this.callback === undefined) {
      if (typeof process !== 'undefined' && process.stdout && process.stderr) {
        ;(stream === 'stdout' ? process.stdout : process.stderr).write(text)
      } else {
        ;(stream === 'stdout' ? console.log : console.error)(text)
      }
      return
    }
    try {
      this.callback(stream, text)
    } catch (err) {
      // Captured and re-thrown at the turn boundary: this function is called
      // from the native binding's threadsafe-function bridge, where a throw
      // would be an unhandled error rather than failing the feed.
      this.failed = true
      this.failure = err
    }
  }

  /** Print failures take precedence over the turn's own outcome. */
  throwIfFailed(): void {
    if (this.failed) {
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
  /** Captured before the first feed/load await and shared by the snapshot chain, never serialized. */
  readonly traceBaseContext = ContextAPI.active()

  /** Exposed so the session's first turn can stream prints through it. */
  get onPrint(): PrintCallback {
    return bindPrintCallback(this.printTarget.write.bind(this.printTarget))
  }

  constructor(
    private readonly native: NativeSession,
    /** The owning session's instance registry, shared so snapshot resumes see
     *  wrappers sent by earlier feeds. Read by the snapshot classes. */
    readonly instances: InstanceStore,
    private readonly printTarget: PrintTarget,
    private readonly answerer: TurnAnswerer,
    private readonly poison: (err: Error) => Error,
  ) {}

  /** Resolves a turn to the next snapshot, answering nothing automatically. */
  async advance(turn: NativeTurn): Promise<Snapshot> {
    for (;;) {
      switch (turn.kind) {
        case 'complete':
          this.printTarget.throwIfFailed()
          return new MontyComplete(restore(turn.value, this.instances))
        case 'error':
          this.printTarget.throwIfFailed()
          throw montyErrorFromNative(turn.exception)
        case 'typingError':
          this.printTarget.throwIfFailed()
          throw new MontyTypingError(turn.diagnostics)
        case 'crashed':
        case 'disconnected':
        case 'shutdown':
          throw this.poison(lostSessionError(turn))
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

  /** Whether host promises remain for automatic resolution. */
  get hasPendingFutures(): boolean {
    return this.answerer.futures.size > 0
  }

  // resume primitives — each runs one turn, then advances to the next snapshot

  async resumeReturn(value: unknown): Promise<Snapshot> {
    // a prepare rejection surfaces as an in-sandbox error, like a native
    // conversion failure would
    let outbound: unknown
    try {
      outbound = prepare(value, this.instances)
    } catch (err) {
      return this.resumeError(err)
    }
    return this.advance((await this.native.resumeReturn(outbound, this.onPrint)) as NativeTurn)
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
    return this.advance((await this.native.resumeNameLookup(functionName, null, this.onPrint)) as NativeTurn)
  }

  async resumeNameLookupValue(value: unknown): Promise<Snapshot> {
    return this.advance(
      (await this.native.resumeNameLookup(null, { value: prepare(value, this.instances) }, this.onPrint)) as NativeTurn,
    )
  }

  async resolveFutures(results: NativeFutureResult[]): Promise<Snapshot> {
    const next = (await this.native.resolveFutures(results, this.onPrint)) as NativeTurn
    for (const result of results) {
      this.answerer.futures.delete(result.callId)
    }
    return this.advance(next)
  }

  async dump(): Promise<Buffer> {
    return dumpSession(this.native, this.onPrint, this.poison)
  }
}

/** Shares tracing context access and the single-resume check across snapshot kinds. */
class SingleUse {
  private used = false

  /** Retains the suspension's telemetry parent until the snapshot is resumed. */
  protected constructor(
    private readonly callbackSpanKey: string | undefined,
    private readonly traceBaseContext: Context,
  ) {}

  /**
   * Returns the suspension's OTel context without activating it or resuming execution.
   * Preserves context entries captured at `feedStart` / `loadSnapshot`; without Monty tracing,
   * returns that captured context unchanged. Access after resume throws; previously returned
   * contexts remain usable but do not keep the suspension span open.
   */
  traceContext(): Context {
    this.ensureUnused()
    return getCallbackContext(this.callbackSpanKey, this.traceBaseContext)
  }

  /** Consumes the snapshot's single resume, independently of its tracing context. */
  protected claim(): void {
    this.ensureUnused()
    this.used = true
  }

  /** Rejects operations on a suspension the caller has already answered. */
  protected ensureUnused(): void {
    if (this.used) {
      throw new Error('snapshot has already been resumed')
    }
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
  /** `resumeAuto` may await a coroutine directly at this suspension. */
  readonly allowEagerAwait: boolean
  /** Set for host-routed calls: the receiver's store uuid — a class
   *  instance, or a class type (a classmethod, or `__call__` construction).
   *  The receiver is not in `args`; `null` for plain external calls. */
  readonly objectId: string | null
  /** Where the call expression is in the source. */
  readonly position: SourceRange

  /** @internal */
  constructor(
    private readonly driver: SnapshotDriver,
    private readonly turn: FunctionCallTurn | OsCallTurn,
    isOsFunction: boolean,
  ) {
    super(turn.callbackSpanKey, driver.traceBaseContext)
    this.position = turn.position
    this.functionName = turn.functionName
    const [args, kwargs] = restoreCallArgs(turn, driver.instances)
    this.args = args
    this.kwargs = kwargsToRecord(kwargs)
    this.callId = turn.callId
    this.isOsFunction = isOsFunction
    this.allowEagerAwait = turn.allowEagerAwait ?? false
    this.objectId = 'objectId' in turn ? (turn.objectId ?? null) : null
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
   * raise `NameError`. Eligible promises are awaited directly; others settle
   * later through [`FutureSnapshot.resumeAuto`]. Resumes at most once.
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
  /** Set for lazy attribute lookups on a host-backed object (a class
   *  instance, or a class type): the receiver's store uuid. `null` for
   *  plain name lookups. */
  readonly objectId: string | null
  /** Where the name (or attribute access) is in the source. */
  readonly position: SourceRange

  /** @internal */
  constructor(
    private readonly driver: SnapshotDriver,
    private readonly turn: NameLookupTurn,
  ) {
    super(turn.callbackSpanKey, driver.traceBaseContext)
    this.position = turn.position
    this.variableName = turn.name
    this.objectId = turn.objectId ?? null
  }

  /** Resolves the name to an external function by name, or — with no
   *  argument — leaves it unresolved: the sandbox raises `NameError` for a
   *  plain lookup, `AttributeError` when `objectId` is set. */
  resume(functionName?: string): Promise<Snapshot> {
    this.claim()
    return this.driver.resumeNameLookup(functionName ?? null)
  }

  /** Resolves the lookup to `value` directly (any convertible value,
   *  including `null`/`undefined` → `None`) — the way to answer a lazy
   *  `objectId` attribute lookup by hand, and the counterpart of
   *  pydantic_monty's `resume(value=...)`. Resumes at most once. */
  resumeValue(value: unknown): Promise<Snapshot> {
    this.claim()
    return this.driver.resumeNameLookupValue(value)
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
  /** Where the main task's blocked `await` is in the source. */
  readonly position: SourceRange

  /** @internal */
  constructor(
    private readonly driver: SnapshotDriver,
    private readonly turn: ResolveFuturesTurn,
  ) {
    super(turn.callbackSpanKey, driver.traceBaseContext)
    this.position = turn.position
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
      // a prepare rejection is delivered as the future's error, like a sync
      // return value's conversion failure
      try {
        return { callId: result.callId, ok: true, value: prepare(result.value, this.driver.instances) }
      } catch (err) {
        const { excType, message } = jsErrorParts(err)
        return { callId: result.callId, ok: false, excType, message }
      }
    })
    return this.driver.resolveFutures(native)
  }

  /**
   * Waits for registered host promises, delivers their outcomes, and returns
   * the next snapshot. With no registered promises (e.g. after `loadSnapshot`),
   * rejects without consuming the snapshot or invalidating the session;
   * use `resume([...])` instead. Once automatic resolution starts, the snapshot
   * is consumed even if it fails.
   */
  resumeAuto(): Promise<Snapshot> {
    this.ensureUnused()
    if (this.turn.pendingCallIds.length > 0 && !this.driver.hasPendingFutures) {
      return Promise.reject(new Error('no pending host promises to resume; use resume([...])'))
    }
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

/** The OS call the sandbox makes for an `import` it cannot resolve itself. */
const IMPORT_CALL = 'import'

/** The OS call a type-checked feed makes for the stubs of the host modules it imports. */
const IMPORT_STUBS_CALL = 'import_stubs'

/** Modules keyed by import name, read when a snippet first imports one: invalid identifiers, keywords
 *  and sandbox-provided names throw there. Factories run at most once per session and may return a Promise. */
export type ExternalModules = Record<string, ExternalModule | (() => ExternalModule | Promise<ExternalModule>)>

/**
 * A session's modules and cached factory results. The checkout's record is read
 * live and an entry is validated when a snippet first reaches it, so the host may
 * add modules during the session.
 * @internal
 */
export class HostModules {
  /** Factory results cached for this session. */
  readonly resolved = new Map<string, ExternalModule>()

  /** The checkout's record, kept by reference so later additions are importable. */
  private readonly entries: ExternalModules

  constructor(entries: ExternalModules | undefined) {
    const proto = entries !== undefined && typeof entries === 'object' ? Object.getPrototypeOf(entries) : undefined
    if (entries !== undefined && proto !== Object.prototype && proto !== null) {
      throw new TypeError(
        `externalModules must be a plain object of module name to ExternalModule, got ${kindOf(entries)}`,
      )
    }
    this.entries = entries ?? {}
  }

  /** Checks and caches a factory result. */
  install(name: string, returned: unknown): ExternalModule {
    if (!(returned instanceof ExternalModule)) {
      throw new TypeError(`externalModules.${name}() returned ${kindOf(returned)}, not an ExternalModule`)
    }
    this.resolved.set(name, returned)
    return returned
  }

  /**
   * The top-level entry for `name` (own keys only), validated now that a snippet
   * reaches it: a module name the sandbox does not provide, mapped to an
   * `ExternalModule` or a factory.
   */
  entry(name: string): ExternalModules[string] | undefined {
    // an own key holding `undefined` is a bad entry, not an absent one
    if (!Object.prototype.hasOwnProperty.call(this.entries, name)) {
      return undefined
    }
    const entry = this.entries[name] as unknown
    checkModuleName(`externalModules.${name}`, name)
    if (!(entry instanceof ExternalModule) && typeof entry !== 'function') {
      throw new TypeError(`externalModules.${name} must be an ExternalModule or a function returning one`)
    }
    return entry as ExternalModules[string]
  }
}

/** `value` wrapped so an async return cannot await it; `undefined` stays absent. */
function boxed(value: unknown): { value: unknown } | undefined {
  return value === undefined ? undefined : { value }
}

function ownEntry(record: unknown, key: string): unknown {
  return record !== null && typeof record === 'object' && Object.prototype.hasOwnProperty.call(record, key)
    ? (record as Record<string, unknown>)[key]
    : undefined
}

/** The submodule `name` of `module`: the `ExternalModule` under that key, if any. */
function submodule(module: ExternalModule, name: string): ExternalModule | undefined {
  const value = ownEntry(module.attrs, name)
  return value instanceof ExternalModule ? value : undefined
}

/** A module's attributes in order (all public: the module never holds a `_` name). */
function publicAttrs(module: ExternalModule): [string, unknown][] {
  return Object.keys(module.attrs).map((key) => [key, module.attrs[key]])
}

/** Collects stubs by dotted path, using `null` for modules without stubs. */
function collectStubs(path: string, entry: ExternalModule, stubs: Record<string, string | null>): void {
  stubs[path] = entry.stubs ?? null
  for (const [key, value] of publicAttrs(entry)) {
    if (value instanceof ExternalModule) {
      collectStubs(`${path}.${key}`, value, stubs)
    }
  }
}

/**
 * The module value an import binds: the module's public attrs, with functions
 * as host functions named by their dotted path (which [`TurnAnswerer.hostEntry`]
 * resolves back through the module) and nested modules as submodules.
 */
function moduleValue(path: string, entry: ExternalModule): HostModuleValue {
  const attrs = publicAttrs(entry).map(([key, value]): [string, unknown] => {
    const sent =
      value instanceof ExternalModule
        ? moduleValue(`${path}.${key}`, value)
        : typeof value === 'function'
          ? namedHostFunction(`${path}.${key}`, value as ExternalFunction)
          : value
    return [key, sent]
  })
  return new HostModuleValue(path, entry.id, attrs)
}

function namedHostFunction(name: string, fn: ExternalFunction): ExternalFunction {
  const proxy: ExternalFunction = (...args: unknown[]) => fn(...(args as never[]))
  Object.defineProperty(proxy, 'name', { value: name })
  return proxy
}

/** Positional args, with kwargs appended as an object when present. */
function buildCallArgs(args: unknown[], kwargs: [unknown, unknown][]): unknown[] {
  if (kwargs.length === 0) {
    return args
  }
  return [...args, kwargsToRecord(kwargs)]
}

/** Prepares each input value for the wire (feed's `inputs` record) with one
 *  memo, so an object passed under two names is one sandbox object. */
function prepareInputs(
  inputs: Record<string, unknown> | undefined,
  store: InstanceStore,
): Record<string, unknown> | null {
  if (inputs === undefined) {
    return null
  }
  // null prototype so an exotic input name (e.g. `__proto__`) stays a plain
  // property of the rebuilt record
  const prepared: Record<string, unknown> = Object.create(null)
  const memo: WalkMemo = new Map()
  let changed = false
  for (const [name, value] of Object.entries(inputs)) {
    prepared[name] = prepare(value, store, memo)
    changed ||= prepared[name] !== value
  }
  return changed ? prepared : inputs
}

/** Restores a call's args and the values of its `[key, value]` kwarg pairs
 *  with one memo, so an object the sandbox passed twice is one host object. */
function restoreCallArgs(
  call: { args: unknown[]; kwargs: [unknown, unknown][] },
  store: InstanceStore,
): [unknown[], [unknown, unknown][]] {
  const memo: WalkMemo = new Map()
  const args = call.args.map((value) => restore(value, store, memo))
  const kwargs = call.kwargs.map(([key, value]): [unknown, unknown] => [key, restore(value, store, memo)])
  return [args, kwargs]
}

/**
 * Converts `[key, value]` kwarg pairs into a record (string keys only). The
 * record has a null prototype and a `__proto__` key is dropped: keys are
 * sandbox-controlled, and a host copying the record with `Object.assign`
 * or `{...kwargs}` onto an ordinary object must not have its prototype
 * replaced.
 */
function kwargsToRecord(pairs: [unknown, unknown][]): Record<string, unknown> {
  const kwargs: Record<string, unknown> = Object.create(null)
  for (const [key, value] of pairs) {
    if (typeof key === 'string' && key !== '__proto__') {
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

/**
 * Checks `native` out of its pool, raising what a turn would for the same
 * failure: `MontyRuntimeError` when the server refuses the session,
 * `MontyCrashedError` / `MontyDisconnectError` / `MontyShutdown` when the worker
 * is lost, and a `TimeoutError` `DOMException` when no worker became available
 * within the checkout timeout. Anything else (a failed spawn or dial) is a plain `Error`.
 */
export async function enterSession(
  native: NativeSession,
  telemetryContext: unknown,
  connectHeaders?: { name: string; value: string }[],
): Promise<void> {
  const turn = (await native.enter(telemetryContext, connectHeaders)) as EnterTurn
  switch (turn.kind) {
    case 'ok':
      return
    case 'exhausted':
      throw new DOMException(turn.message, 'TimeoutError')
    case 'error':
      throw montyErrorFromNative(turn.exception)
    case 'crashed':
    case 'disconnected':
    case 'shutdown':
      throw lostSessionError(turn)
    case 'typingError':
      throw new Error(turn.diagnostics)
    case 'protocol':
      throw new Error(turn.message)
  }
}

/**
 * The error for a turn that lost the session's worker: a crash, a dropped
 * remote connection, or a remote server shutting down.
 */
function lostSessionError(turn: CrashedTurn | DisconnectedTurn | ShutdownTurn): MontyError {
  switch (turn.kind) {
    case 'crashed':
      return new MontyCrashedError(turn.message, turn)
    case 'disconnected':
      return new MontyDisconnectError(turn.message)
    case 'shutdown':
      return new MontyShutdown(turn.message, turn.dump === undefined ? null : bufferFrom(turn.dump))
  }
}

/**
 * Runs a dump turn, resolving to the session's state or a storing server's
 * record ID. A refused dump throws `MontyRuntimeError` and leaves the session
 * usable; a lost worker or protocol failure poisons it.
 */
async function dumpSession(
  native: NativeSession,
  onPrint: PrintCallback,
  poison: (err: Error) => Error,
): Promise<Buffer> {
  const turn = (await native.dump(onPrint)) as NativeTurn | DumpedTurn
  switch (turn.kind) {
    case 'dumped':
      return bufferFrom(turn.state)
    case 'error':
      throw montyErrorFromNative(turn.exception)
    case 'crashed':
    case 'disconnected':
    case 'shutdown':
      throw poison(lostSessionError(turn))
    case 'protocol':
      throw poison(new ProtocolError(turn.message))
    default:
      throw poison(new ProtocolError(`unexpected turn kind: ${(turn as { kind: string }).kind}`))
  }
}

function bytesForNative(bytes: Uint8Array): Buffer {
  return (typeof Buffer === 'undefined' ? bytes : Buffer.from(bytes)) as Buffer
}

function bufferFrom(bytes: Uint8Array): Buffer {
  return (typeof Buffer === 'undefined' ? bytes : Buffer.from(bytes)) as Buffer
}

/**
 * Resolves after `ms` milliseconds. `setTimeout` takes a signed 32-bit
 * millisecond count, so a longer wait is chained rather than cut short.
 */
async function sleepMs(ms: number): Promise<void> {
  const MAX_TIMEOUT_MS = 2 ** 31 - 1
  let left = ms
  while (left > MAX_TIMEOUT_MS) {
    await new Promise<void>((resolve) => setTimeout(resolve, MAX_TIMEOUT_MS))
    left -= MAX_TIMEOUT_MS
  }
  await new Promise<void>((resolve) => setTimeout(resolve, left))
}
