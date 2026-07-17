// The wasm worker transport: a structural stand-in for `NativeSession`.
//
// `MontySession`'s drive loop (`session.ts`) is written against a small set of
// methods on its `native` object — `feed`, the `resume*` family,
// `resolveFutures`, `dump`, `finish`, `workerPid`. The napi pool implements
// them in Rust; this implements the same shape over the wasm worker, encoding
// `ParentRequest`s and decoding `ChildEvent`s in TypeScript so the loop runs
// unchanged. Inject it via `new MontySession(transport)`.
//
// Differences from the native path, all acceptable for the async-only browser
// model: `Print`s are buffered per turn (the worker hands back a turn's frames
// together) and flushed through `onPrint` before the turn resolves; error
// `traceback` strings are not yet rendered (frames are decoded; the rendered
// string is a follow-up); mounts are rejected (no host filesystem in a worker).

import type { NativeException, NativeFrame, NativeFutureResult, NativeTurn } from '../native.js'
import { type AssertMessageAnnotations, encodeAssertMessageAnnotations } from '../options.js'
import type { Dispatcher } from './host.js'
import { Reader, Wire, Writer, deframe, frame } from './proto.js'
import { decodeMontyObject, encodeMontyObject } from './value.js'

type OnPrint = (stream: 'stdout' | 'stderr', text: string) => void

/** Resource limits enforced inside the worker, mirroring the napi pool's. */
export interface ResourceLimits {
  maxAllocations?: number
  maxDurationSecs?: number
  maxMemory?: number
  gcInterval?: number
  maxRecursionDepth?: number
}

/** Session-creation options sent in the `ReplCreate` request. */
export interface WorkerSessionConfig {
  scriptName?: string
  limits?: ResourceLimits
  typeCheck?: boolean
  typeCheckStubs?: string
  /**
   * Give failed `assert`s introspected messages. Absent/true means the
   * child's default (a 120-byte operand-repr truncation), false turns them
   * off, an integer >= 1 customizes the truncation length.
   */
  assertMessageAnnotations?: AssertMessageAnnotations
}

// ParentRequest oneof field numbers (see proto/monty/v1/monty.proto). Note
// field 2 is InstallDependencies (CPython-only) and field 8 is Load, neither of
// which the wasm transport sends — hence the gaps.
const Req = {
  ReplCreate: 1,
  ReplFeed: 3,
  ResumeCall: 4,
  ResumeNameLookup: 5,
  ResumeFutures: 6,
  Dump: 7,
  Load: 8,
  Reset: 9,
}
// ChildEvent oneof field numbers.
const Ev = {
  Print: 1,
  FunctionCall: 2,
  OsCall: 3,
  NameLookup: 4,
  ResolveFutures: 5,
  Complete: 6,
  Error: 7,
  TypingError: 8,
  DumpResult: 9,
  Ok: 10,
  FatalError: 11,
}

export class WorkerTransport {
  /** The id/name of the suspension awaiting an answer, for the `resume*` family. */
  private pendingCallId = 0
  private pendingFunctionName = ''
  private pendingNotHandled: { excType: string; message: string } | null = null

  /** No OS process backs a wasm worker. */
  readonly workerPid: number | null = null

  /**
   * Set once a turn reveals the worker is dead (crash/channel error): `finish`
   * then skips `Reset` and the owner reclaims via `onFinish(false)`.
   */
  private dead = false

  /**
   * Invoked when the session ends, telling the owner (a pool) whether the
   * worker may be reused. `true` after a clean `Reset`; `false` if the worker
   * died and must be discarded.
   */
  onFinish?: (reusable: boolean) => void

  private constructor(private readonly dispatcher: Dispatcher) {}

  /** Creates the REPL session (`ReplCreate`) and returns the ready transport. */
  static async create(dispatcher: Dispatcher, config: WorkerSessionConfig = {}): Promise<WorkerTransport> {
    const transport = new WorkerTransport(dispatcher)
    const create = new Writer()
    create.string(1, config.scriptName ?? 'main.py') // ReplCreate.script_name
    if (config.limits) create.lengthDelimited(2, encodeLimits(config.limits)) // ReplCreate.limits
    if (config.typeCheck) create.bool(3, true) // ReplCreate.type_check
    if (config.typeCheckStubs !== undefined) create.string(4, config.typeCheckStubs) // ReplCreate.type_check_stubs
    // Configure.assert_message_annotations (field 6, optional uint32):
    // absent = child default (on, 120-byte truncation), 0 = off, n = custom.
    const assertAnnotations = encodeAssertMessageAnnotations(config.assertMessageAnnotations)
    if (assertAnnotations !== undefined) create.uint(6, assertAnnotations)
    await transport.control(Req.ReplCreate, create.finish(), Ev.Ok, 'ReplCreate')
    return transport
  }

  feed(
    code: string,
    inputs: Record<string, unknown> | null,
    mounts: readonly unknown[],
    skipTypeCheck: boolean,
    onPrint: OnPrint,
  ): Promise<NativeTurn> {
    if (mounts.length > 0) {
      throw new Error('the wasm worker does not support filesystem mounts (browser has no host filesystem)')
    }
    const feed = new Writer()
    feed.string(1, code) // ReplFeed.code
    for (const [name, value] of Object.entries(inputs ?? {})) {
      const named = new Writer()
      named.string(1, name) // NamedValue.name
      named.lengthDelimited(2, encodeMontyObject(value)) // NamedValue.value
      feed.lengthDelimited(2, named.finish()) // ReplFeed.inputs
    }
    if (skipTypeCheck) feed.bool(4, true) // ReplFeed.skip_type_check
    return this.turn(Req.ReplFeed, feed.finish(), onPrint)
  }

  resumeReturn(value: unknown, onPrint: OnPrint): Promise<NativeTurn> {
    return this.resumeCall(encodeExtReturn(value), onPrint)
  }

  resumeError(excType: string, message: string, onPrint: OnPrint): Promise<NativeTurn> {
    return this.resumeCall(extResult(2, raisedException(excType, message)), onPrint) // ExtFunctionResult.error
  }

  resumeNotFound(onPrint: OnPrint): Promise<NativeTurn> {
    const notFound = new Writer()
    notFound.string(4, this.pendingFunctionName) // ExtFunctionResult.not_found
    return this.resumeCall(notFound.finish(), onPrint)
  }

  resumeNotHandled(onPrint: OnPrint): Promise<NativeTurn> {
    const exc = this.pendingNotHandled ?? { excType: 'RuntimeError', message: 'OS call is not supported' }
    return this.resumeCall(extResult(2, raisedException(exc.excType, exc.message)), onPrint)
  }

  resumeFuture(onPrint: OnPrint): Promise<NativeTurn> {
    const future = new Writer()
    future.uint(3, this.pendingCallId) // ExtFunctionResult.future = call_id
    return this.resumeCall(future.finish(), onPrint)
  }

  resumeNameLookup(
    functionName: string | null,
    value: { value: unknown } | null,
    onPrint: OnPrint,
  ): Promise<NativeTurn> {
    const lookup = new Writer()
    if (functionName !== null) {
      lookup.lengthDelimited(1, functionValue(functionName)) // ResumeNameLookup.value
    } else if (value !== null) {
      lookup.lengthDelimited(1, encodeMontyObject(value.value)) // ResumeNameLookup.value
    } else {
      lookup.lengthDelimited(2, EMPTY) // ResumeNameLookup.undefined = Unit
    }
    return this.turn(Req.ResumeNameLookup, lookup.finish(), onPrint)
  }

  async installDependencies(requirements: string[], _onPrint: OnPrint): Promise<NativeTurn | { kind: 'ok' }> {
    if (requirements.length === 0) {
      return { kind: 'ok' }
    }
    return {
      kind: 'error',
      exception: {
        excType: 'RuntimeError',
        message: 'dependency installation is only supported by the CPython worker',
        traceback: '',
        frames: [],
      },
    }
  }

  resolveFutures(results: NativeFutureResult[], onPrint: OnPrint): Promise<NativeTurn> {
    const futures = new Writer()
    for (const r of results) {
      const result = new Writer()
      result.uint(1, r.callId) // FutureResult.call_id
      const kind = r.ok
        ? extResult(1, encodeMontyObject(r.value))
        : extResult(2, raisedException(r.excType ?? 'RuntimeError', r.message ?? ''))
      result.lengthDelimited(2, kind) // FutureResult.result
      futures.lengthDelimited(1, result.finish()) // ResumeFutures.results
    }
    return this.turn(Req.ResumeFutures, futures.finish(), onPrint)
  }

  async dump(): Promise<Uint8Array> {
    const event = await this.control(Req.Dump, EMPTY, Ev.DumpResult, 'Dump')
    const reader = new Reader(event.bytes)
    while (!reader.done) {
      const f = reader.next()
      if (f.field === 1) return f.bytes // DumpResult.state
    }
    throw new Error('DumpResult carried no state')
  }

  async restore(
    state: Uint8Array,
    mounts: readonly unknown[],
    onPrint: OnPrint,
  ): Promise<NativeTurn | { kind: 'loaded' }> {
    if (mounts.length > 0) {
      throw new Error('the wasm worker does not support filesystem mounts (browser has no host filesystem)')
    }
    const load = new Writer()
    load.bytes(1, state) // Load.state
    const event = await this.run(Req.Load, load.finish(), onPrint)
    if (!event) return crashed('worker exited without a turn-ending event')
    if (event.kind === Ev.Ok) return { kind: 'loaded' }
    return this.toTurn(event)
  }

  /**
   * Ends the session. A live worker is `Reset` and reported reusable; a dead
   * one is reported for disposal. Never throws — `MontySession.close` relies on
   * that even after a crash.
   */
  async finish(): Promise<void> {
    if (this.dead) {
      this.onFinish?.(false)
      return
    }
    try {
      await this.control(Req.Reset, EMPTY, Ev.Ok, 'Reset')
      this.onFinish?.(true)
    } catch {
      this.dead = true
      this.onFinish?.(false)
    }
  }

  // === turn plumbing ===

  private resumeCall(extFunctionResult: Uint8Array, onPrint: OnPrint): Promise<NativeTurn> {
    const call = new Writer()
    call.uint(1, this.pendingCallId) // ResumeCall.call_id
    call.lengthDelimited(2, extFunctionResult) // ResumeCall.result
    return this.turn(Req.ResumeCall, call.finish(), onPrint)
  }

  /** Sends a request and decodes the terminating event into a `NativeTurn`. */
  private async turn(field: number, payload: Uint8Array, onPrint: OnPrint): Promise<NativeTurn> {
    const event = await this.run(field, payload, onPrint)
    const turn = event ? this.toTurn(event) : crashed('worker exited without a turn-ending event')
    if (turn.kind === 'crashed') this.dead = true
    return turn
  }

  /** Sends a control request (ReplCreate/Reset/Dump) and asserts its event kind. */
  private async control(field: number, payload: Uint8Array, kind: number, what: string): Promise<ChildEventFrame> {
    const event = await this.run(field, payload, undefined)
    if (!event) throw new Error(`${what} produced no turn-ending event (worker crashed)`)
    if (event.kind !== kind) throw new Error(`${what} expected event ${kind}, got ${event.kind}`)
    return event
  }

  /** Runs one turn, streaming `Print`s, and returns the single terminating event. */
  private async run(field: number, payload: Uint8Array, onPrint: OnPrint | undefined): Promise<ChildEventFrame | null> {
    const request = new Writer()
    request.lengthDelimited(field, payload) // ParentRequest oneof
    let reply: Uint8Array
    let decodedEvents: ChildEventFrame[] | undefined
    try {
      const dispatched = await this.dispatcher(frame(request.finish()))
      reply = dispatched.reply
      decodedEvents = dispatched.events?.map((event) => ({ kind: event.kind, bytes: Uint8Array.from(event.bytes) }))
    } catch {
      // the dispatcher rejects when the worker died or the channel broke; the
      // caller treats a missing terminating event as a crash
      return null
    }
    let terminating: ChildEventFrame | null = null
    for (const event of decodedEvents ?? decodeChildEvents(reply)) {
      if (event.kind === Ev.Print) {
        if (onPrint) {
          const [stream, text] = decodePrint(event.bytes)
          onPrint(stream, text)
        }
      } else {
        terminating = event
      }
    }
    return terminating
  }

  private toTurn(event: ChildEventFrame): NativeTurn {
    switch (event.kind) {
      case Ev.Complete:
        return { kind: 'complete', value: decodeComplete(event.bytes) }
      case Ev.Error:
        return { kind: 'error', exception: decodeError(event.bytes) }
      case Ev.TypingError:
        return { kind: 'typingError', diagnostics: decodeSingleString(event.bytes) }
      case Ev.FunctionCall: {
        const call = decodeCall(event.bytes, false)
        this.pendingCallId = call.callId
        this.pendingFunctionName = call.functionName
        this.pendingNotHandled = null
        return {
          kind: 'functionCall',
          functionName: call.functionName,
          args: call.args,
          kwargs: call.kwargs,
          callId: call.callId,
          methodCall: call.methodCall,
        }
      }
      case Ev.OsCall: {
        const call = decodeCall(event.bytes, true)
        this.pendingCallId = call.callId
        this.pendingFunctionName = call.functionName
        this.pendingNotHandled = call.notHandledError ? simpleExc(call.notHandledError) : null
        return {
          kind: 'osCall',
          functionName: call.functionName,
          args: call.args,
          kwargs: call.kwargs,
          callId: call.callId,
          notHandledError: call.notHandledError,
        }
      }
      case Ev.NameLookup:
        return { kind: 'nameLookup', name: decodeSingleString(event.bytes) }
      case Ev.ResolveFutures:
        return { kind: 'resolveFutures', pendingCallIds: decodeResolveFutures(event.bytes) }
      case Ev.FatalError:
        return crashed(decodeSingleString(event.bytes))
      default:
        return { kind: 'protocol', message: `unexpected event kind ${event.kind}` }
    }
  }
}

// === ChildEvent / value decoding ===

interface ChildEventFrame {
  readonly kind: number
  readonly bytes: Uint8Array
}

function decodeChildEvents(reply: Uint8Array): ChildEventFrame[] {
  return [...deframe(reply)].map(readChildEvent)
}

/** Extracts the single oneof kind (1..=11) from a `ChildEvent`, ignoring timing. */
function readChildEvent(frameBytes: Uint8Array): ChildEventFrame {
  const reader = new Reader(frameBytes)
  let event: ChildEventFrame | null = null
  while (!reader.done) {
    const f = reader.next()
    if (f.field >= 1 && f.field <= 11) event = { kind: f.field, bytes: f.bytes }
  }
  if (!event) throw new Error('ChildEvent carried no kind')
  return event
}

function decodeComplete(bytes: Uint8Array): unknown {
  const reader = new Reader(bytes)
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 1) return decodeMontyObject(f.bytes) // Complete.value
  }
  return null
}

function decodeError(bytes: Uint8Array): NativeException {
  const reader = new Reader(bytes)
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 1) return decodeRaisedException(f.bytes) // Error.exception
  }
  throw new Error('Error event carried no exception')
}

interface DecodedCall {
  functionName: string
  args: unknown[]
  kwargs: [unknown, unknown][]
  callId: number
  methodCall: boolean
  notHandledError?: NativeException
}

/** Decodes a `FunctionCall` (field 5 = method_call) or `OsCall` (field 5 = not_handled_error). */
function decodeCall(bytes: Uint8Array, isOsCall: boolean): DecodedCall {
  const reader = new Reader(bytes)
  const call: DecodedCall = { functionName: '', args: [], kwargs: [], callId: 0, methodCall: false }
  while (!reader.done) {
    const f = reader.next()
    switch (f.field) {
      case 1:
        call.functionName = decodeString(f.bytes)
        break
      case 2:
        call.args.push(decodeMontyObject(f.bytes))
        break
      case 3:
        call.kwargs.push(decodePair(f.bytes))
        break
      case 4:
        call.callId = Number(f.value)
        break
      case 5:
        if (isOsCall) call.notHandledError = decodeRaisedException(f.bytes)
        else call.methodCall = f.value !== 0n
        break
    }
  }
  return call
}

function decodePair(bytes: Uint8Array): [unknown, unknown] {
  const reader = new Reader(bytes)
  let key: unknown = null
  let value: unknown = null
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 1) key = decodeMontyObject(f.bytes)
    else if (f.field === 2) value = decodeMontyObject(f.bytes)
  }
  return [key, value]
}

function decodeResolveFutures(bytes: Uint8Array): number[] {
  const reader = new Reader(bytes)
  const ids: number[] = []
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 1 && f.wire === Wire.Varint) {
      ids.push(Number(f.value)) // ResolveFutures.pending_call_ids
    } else if (f.field === 1 && f.wire === Wire.LengthDelimited) {
      const packed = new Reader(f.bytes)
      while (!packed.done) ids.push(Number(packed.nextVarint()))
    }
  }
  return ids
}

function decodePrint(bytes: Uint8Array): ['stdout' | 'stderr', string] {
  const reader = new Reader(bytes)
  let stream = 1
  let text = ''
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 1)
      stream = Number(f.value) // Print.stream
    else if (f.field === 2) text = decodeString(f.bytes) // Print.text
  }
  return [stream === 2 ? 'stderr' : 'stdout', text]
}

function decodeRaisedException(bytes: Uint8Array): NativeException {
  const reader = new Reader(bytes)
  let excType = ''
  let message = ''
  const frames: NativeFrame[] = []
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 1) excType = decodeString(f.bytes)
    else if (f.field === 2) message = decodeString(f.bytes)
    else if (f.field === 3) frames.push(decodeStackFrame(f.bytes)) // repeated StackFrame
  }
  // The worker-rendered traceback string is a follow-up; frames are exact.
  return { excType, message, traceback: '', frames }
}

function decodeStackFrame(bytes: Uint8Array): NativeFrame {
  const reader = new Reader(bytes)
  const frame: NativeFrame = {
    filename: '',
    line: 0,
    column: 0,
    endLine: 0,
    endColumn: 0,
    hideCaret: false,
    hideFrameName: false,
  }
  while (!reader.done) {
    const f = reader.next()
    switch (f.field) {
      case 1:
        frame.filename = decodeString(f.bytes)
        break
      case 2:
        ;[frame.line, frame.column] = decodeCodeLoc(f.bytes)
        break
      case 3:
        ;[frame.endLine, frame.endColumn] = decodeCodeLoc(f.bytes)
        break
      case 4:
        frame.frameName = decodeString(f.bytes)
        break
      case 5:
        frame.previewLine = decodeString(f.bytes)
        break
      case 6:
        frame.hideCaret = f.value !== 0n
        break
      case 7:
        frame.hideFrameName = f.value !== 0n
        break
    }
  }
  return frame
}

function decodeCodeLoc(bytes: Uint8Array): [number, number] {
  const reader = new Reader(bytes)
  let line = 0
  let column = 0
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 1) line = Number(f.value)
    else if (f.field === 2) column = Number(f.value)
  }
  return [line, column]
}

function decodeSingleString(bytes: Uint8Array): string {
  const reader = new Reader(bytes)
  while (!reader.done) {
    const f = reader.next()
    if (f.field === 1) return decodeString(f.bytes)
  }
  return ''
}

// === request value helpers ===

/** Encodes a `ResourceLimits` message (durations are seconds -> microseconds). */
function encodeLimits(limits: ResourceLimits): Uint8Array {
  const w = new Writer()
  if (limits.maxAllocations !== undefined) w.uint(1, limits.maxAllocations) // max_allocations
  if (limits.maxDurationSecs !== undefined) w.uint(2, Math.round(limits.maxDurationSecs * 1_000_000)) // max_duration_micros
  if (limits.maxMemory !== undefined) w.uint(3, limits.maxMemory) // max_memory_bytes
  if (limits.gcInterval !== undefined) w.uint(4, limits.gcInterval) // gc_interval
  if (limits.maxRecursionDepth !== undefined) w.uint(5, limits.maxRecursionDepth) // max_recursion_depth
  return w.finish()
}

/** Wraps a payload as one `ExtFunctionResult` oneof field. */
function extResult(field: number, payload: Uint8Array): Uint8Array {
  const w = new Writer()
  w.lengthDelimited(field, payload)
  return w.finish()
}

function raisedException(excType: string, message: string): Uint8Array {
  const w = new Writer()
  w.string(1, excType) // RaisedException.exc_type
  w.string(2, message) // RaisedException.message
  return w.finish()
}

function encodeExtReturn(value: unknown): Uint8Array {
  try {
    return extResult(1, encodeMontyObject(value)) // ExtFunctionResult.return_value
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    return extResult(2, raisedException('TypeError', message))
  }
}

/** A `MontyObject` holding a `Function` value, for answering a name lookup. */
function functionValue(name: string): Uint8Array {
  const fn = new Writer()
  fn.string(1, name) // Function.name
  const obj = new Writer()
  obj.lengthDelimited(25, fn.finish()) // MontyObject.function
  return obj.finish()
}

function simpleExc(exc: NativeException): { excType: string; message: string } {
  return { excType: exc.excType, message: exc.message }
}

function crashed(message: string): NativeTurn {
  return { kind: 'crashed', message, timedOut: false }
}

function decodeString(bytes: Uint8Array): string {
  return new TextDecoder().decode(bytes)
}

const EMPTY = new Uint8Array(0)
