// The wasm worker transport: a structural stand-in for `NativeSession`.
//
// `MontySession` drives the same methods as the napi-backed session, while this
// implementation sends semantic WIT requests to the Rust component. TypeScript
// converts only between public JavaScript values and the component's flat value
// arena; protobuf is now entirely internal to Rust.

import type { NativeFutureResult, NativeTurn, NotMountedTurn } from '../native.js'
import {
  type AssertMessageAnnotations,
  type AutoOsCalls,
  type EncodedAutoOsCalls,
  type EncodedRandomSeed,
  type TypeCheckFormat,
  encodeAssertMessageAnnotations,
  encodeAutoOsCalls,
  encodeTypeCheckFormat,
} from '../options.js'
import type {
  Arena,
  AutoOsCalls as ComponentAutoOsCalls,
  CallResult,
  Event as ComponentEvent,
  NameLookupRequest,
  RandomSeed as ComponentRandomSeed,
  Request as ComponentRequest,
  TimeZone as ComponentTimeZone,
  ResourceLimits as ComponentResourceLimits,
  TypeCheckFormat as ComponentTypeCheckFormat,
} from './component/monty.component.js'
import type { Dispatcher } from './host.js'
import { ArenaEncoder, decodeArena } from './value.js'

/** The arena of a request that carries no value. */
const EMPTY_ARENA: Arena = { nodes: [] }

type OnPrint = (stream: 'stdout' | 'stderr', text: string) => void

/**
 * Encodes a print flush interval (seconds) as whole milliseconds for the WIT
 * `u32`, mirroring `monty-pool`'s `flush_interval_ms`.
 *
 * The component encodes a `u32` as `val >>> 0`, which would silently wrap a
 * negative or non-finite value into a huge interval, so reject those here.
 * Zero is the line-buffering sentinel, so a positive interval never rounds
 * down into it.
 */
function flushIntervalMs(interval: number): number {
  if (!Number.isFinite(interval) || interval < 0) {
    throw new TypeError(`invalid printFlushInterval: expected a non-negative number of seconds, got ${interval}`)
  }
  return interval === 0 ? 0 : Math.min(Math.max(Math.floor(interval * 1000), 1), 0xffffffff)
}

/** Resource limits mirrored from the napi pool; the transport enforces `maxSuspensions`. */
export interface ResourceLimits {
  /**
   * @deprecated Removed: it capped a whole session, which neither replacement
   * does, so there is no value to carry over. Pick `maxFeedDurationSecs` or
   * `maxTurnDurationSecs`. Declared `never` so a stale key still fails to
   * compile rather than being silently dropped at the boundary.
   */
  maxDurationSecs?: never
  maxFeedDurationSecs?: number
  maxTurnDurationSecs?: number
  maxMemory?: number
  gcInterval?: number
  maxRecursionDepth?: number
  maxSuspensions?: number
}

/** Session-creation options sent to the component worker. */
export interface WorkerSessionConfig {
  scriptName?: string
  limits?: ResourceLimits
  typeCheck?: boolean
  typeCheckStubs?: string
  /** How typing diagnostics are rendered by the worker (default `'full'`). */
  typeCheckFormat?: TypeCheckFormat
  /** Render typing diagnostics with ANSI colour escapes (default false). */
  typeCheckColor?: boolean
  /**
   * Give failed `assert`s introspected messages. Absent/true means the
   * child's default, false disables them, and an integer customizes truncation.
   */
  assertMessageAnnotations?: AssertMessageAnnotations
  /**
   * How long, in seconds, the worker may hold buffered `print()` output before
   * sending it (default 0.005). `0` restores line buffering, delivering each
   * completed line on its own. A turn's frames all reach the host together
   * here, but this still sets how they are split: one `printCallback` call per
   * frame, and a print collector charges its `maxBytes` cap per frame.
   */
  printFlushInterval?: number
  /** Which OS calls the worker answers itself; see `AutoOsCalls`. */
  autoOsCalls?: AutoOsCalls
}

/** A session-shaped adapter over one semantic component dispatcher. */
export class WorkerTransport {
  /** The id/name of the suspension awaiting an answer. */
  private pendingCallId = 0
  private pendingFunctionName = ''

  /** No OS process backs a wasm worker. */
  readonly workerPid: number | null = null

  /** Whether a crash or channel error made this worker unreusable. */
  private dead = false

  private suspensionLimit: bigint | undefined
  private suspensionsSeen = 0n

  /** Reports whether the worker can return to its pool when the session ends. */
  onFinish?: (reusable: boolean) => void

  private constructor(private readonly dispatcher: Dispatcher) {}

  /** Creates a configured REPL session over `dispatcher`. */
  static async create(dispatcher: Dispatcher, config: WorkerSessionConfig = {}): Promise<WorkerTransport> {
    const transport = new WorkerTransport(dispatcher)
    const assertMessageAnnotations = encodeAssertMessageAnnotations(config.assertMessageAnnotations)
    const autoOsCalls = componentAutoOsCalls(encodeAutoOsCalls(config.autoOsCalls ?? {}))
    await transport.control(
      {
        tag: 'configure',
        val: {
          scriptName: config.scriptName ?? 'main.py',
          ...(config.limits === undefined ? {} : { limits: encodeLimits(config.limits) }),
          typeCheck: config.typeCheck ?? false,
          ...(config.typeCheckStubs === undefined ? {} : { typeCheckStubs: config.typeCheckStubs }),
          ...(assertMessageAnnotations === undefined ? {} : { assertMessageAnnotations }),
          typeCheckFormat: componentTypeCheckFormat(config.typeCheckFormat ?? 'full'),
          typeCheckColor: config.typeCheckColor ?? false,
          ...(config.printFlushInterval === undefined
            ? {}
            : { printFlushIntervalMs: flushIntervalMs(config.printFlushInterval) }),
          ...(autoOsCalls === undefined ? {} : { autoOsCalls }),
        },
      },
      'ok',
      'Configure',
    )
    return transport
  }

  /** Feeds one snippet and eagerly converts its named inputs. */
  feed(
    code: string,
    inputs: Record<string, unknown> | null,
    mounts: readonly unknown[],
    options: { cwd?: string; skipTypeCheck: boolean },
    onPrint: OnPrint,
  ): Promise<NativeTurn> {
    if (mounts.length > 0) {
      throw new Error('the wasm worker does not support filesystem mounts (browser has no host filesystem)')
    }
    const cwd = feedCwd(options.cwd)
    if (typeof cwd !== 'string') {
      return Promise.resolve(cwd)
    }
    // one arena for every input, so an object passed under two names is one
    // sandbox object
    const encoder = new ArenaEncoder()
    const named = Object.entries(inputs ?? {}).map(([name, value]) => ({ name, value: encoder.push(value) }))
    return this.turn(
      {
        tag: 'feed',
        val: {
          code,
          inputs: named,
          values: encoder.finish(),
          skipTypeCheck: options.skipTypeCheck,
          cwd,
        },
      },
      onPrint,
    )
  }

  /** Resumes the current call with a host return value. */
  resumeReturn(value: unknown, onPrint: OnPrint): Promise<NativeTurn> {
    const [outcome, values] = returnValue(value)
    return this.resumeCall(outcome, onPrint, values)
  }

  /** Resumes the current call by raising a Python exception. */
  resumeError(excType: string, message: string, onPrint: OnPrint): Promise<NativeTurn> {
    return this.resumeCall(errorResult(excType, message), onPrint)
  }

  /** Reports that the current external function name was not provided. */
  resumeNotFound(onPrint: OnPrint): Promise<NativeTurn> {
    return this.resumeCall({ tag: 'not-found', val: this.pendingFunctionName }, onPrint)
  }

  /** Lets the child apply the pending OS call's no-handler semantics. */
  resumeNotHandled(onPrint: OnPrint): Promise<NativeTurn> {
    return this.resumeCall({ tag: 'not-handled' }, onPrint)
  }

  /** A wasm worker has no host filesystem mounts to consult. */
  resumeFromMounts(_onPrint: OnPrint): Promise<NotMountedTurn> {
    return Promise.resolve({ kind: 'notMounted' })
  }

  /** Registers the current call as an external future. */
  resumeFuture(onPrint: OnPrint): Promise<NativeTurn> {
    return this.resumeCall({ tag: 'pending-future', val: this.pendingCallId }, onPrint)
  }

  /** Answers an undefined-name suspension with a function, value, or absence. */
  resumeNameLookup(
    functionName: string | null,
    value: { value: unknown } | null,
    onPrint: OnPrint,
  ): Promise<NativeTurn> {
    let request: NameLookupRequest
    if (functionName !== null) {
      request = {
        outcome: { tag: 'value', val: 0 },
        values: { nodes: [{ tag: 'function', val: { name: functionName } }] },
      }
    } else if (value !== null) {
      const encoder = new ArenaEncoder()
      const root = encoder.push(value.value)
      request = { outcome: { tag: 'value', val: root }, values: encoder.finish() }
    } else {
      request = { outcome: { tag: 'undefined' }, values: EMPTY_ARENA }
    }
    return this.turn({ tag: 'resume-name-lookup', val: request }, onPrint)
  }

  /** Answers a lazy attribute lookup; a value the arena cannot encode raises `TypeError` in the sandbox. */
  resumeLazyAttr(value: unknown, onPrint: OnPrint): Promise<NativeTurn> {
    return this.turn({ tag: 'resume-name-lookup', val: lazyAttrValue(value) }, onPrint)
  }

  /** Answers a name lookup with an exception raised where it suspended. */
  resumeNameLookupError(excType: string, message: string, onPrint: OnPrint): Promise<NativeTurn> {
    const request: NameLookupRequest = { outcome: { tag: 'error', val: { excType, message } }, values: EMPTY_ARENA }
    return this.turn({ tag: 'resume-name-lookup', val: request }, onPrint)
  }

  /** Reports the sandbox worker's lack of dependency installation. */
  async installDependencies(requirements: string[], _onPrint: OnPrint): Promise<NativeTurn | { kind: 'ok' }> {
    return requirements.length === 0
      ? { kind: 'ok' }
      : {
          kind: 'error',
          exception: {
            excType: 'RuntimeError',
            message: 'dependency installation is only supported by the CPython worker',
            traceback: '',
            frames: [],
          },
        }
  }

  /** Delivers settled external futures to the suspended worker, their
   *  values sharing one arena. */
  resolveFutures(results: NativeFutureResult[], onPrint: OnPrint): Promise<NativeTurn> {
    const encoder = new ArenaEncoder()
    const settled = results.map((result) => ({
      callId: result.callId,
      outcome: result.ok
        ? ({ tag: 'return-value', val: encoder.push(result.value) } as CallResult)
        : errorResult(result.excType ?? 'RuntimeError', result.message ?? ''),
    }))
    return this.turn({ tag: 'resume-futures', val: { results: settled, values: encoder.finish() } }, onPrint)
  }

  /** Dumps the current session into opaque bytes. */
  async dump(): Promise<Uint8Array> {
    const event = await this.control({ tag: 'dump' }, 'dump-result', 'Dump')
    if (event.tag === 'dump-result') return event.val
    throw new Error('Dump returned an unexpected event')
  }

  /** Restores a previously dumped session into this fresh worker. */
  async restore(
    state: Uint8Array,
    mounts: readonly unknown[],
    onPrint: OnPrint,
  ): Promise<NativeTurn | { kind: 'loaded' }> {
    if (mounts.length > 0) {
      throw new Error('the wasm worker does not support filesystem mounts (browser has no host filesystem)')
    }
    const event = await this.run({ tag: 'load', val: state }, onPrint)
    if (!event) return crashed('worker exited without a turn-ending event')
    return event.tag === 'ok' ? { kind: 'loaded' } : this.enforceSuspensionLimit(this.toTurn(event), onPrint)
  }

  /** Resets a live worker for reuse and disposes a dead worker. */
  async finish(): Promise<void> {
    if (this.dead) {
      this.onFinish?.(false)
    } else {
      try {
        await this.control({ tag: 'reset' }, 'ok', 'Reset')
        this.onFinish?.(true)
      } catch {
        this.dead = true
        this.onFinish?.(false)
      }
    }
  }

  /** Answers the current function or OS suspension. */
  private resumeCall(outcome: CallResult, onPrint: OnPrint, values: Arena = EMPTY_ARENA): Promise<NativeTurn> {
    return this.turn({ tag: 'resume-call', val: { callId: this.pendingCallId, outcome, values } }, onPrint)
  }

  /** Sends one request and converts its terminating event into a native turn. */
  private async turn(request: ComponentRequest, onPrint: OnPrint): Promise<NativeTurn> {
    const event = await this.run(request, onPrint)
    const turn = event ? this.toTurn(event) : crashed('worker exited without a turn-ending event')
    return this.enforceSuspensionLimit(turn, onPrint)
  }

  /** Counts a suspension and aborts the feed when it exceeds the session limit. */
  private async enforceSuspensionLimit(turn: NativeTurn, onPrint: OnPrint): Promise<NativeTurn> {
    if (isSuspension(turn)) {
      this.suspensionsSeen += 1n
      // Abort instead of exposing an over-budget suspension to the host.
      if (this.suspensionLimit !== undefined && this.suspensionsSeen > this.suspensionLimit) {
        const message = `suspension limit ${this.suspensionLimit} exceeded`
        const aborted = await this.run({ tag: 'abort-feed', val: { excType: 'RuntimeError', message } }, onPrint)
        turn = aborted ? this.toTurn(aborted) : crashed('worker exited without a turn-ending event')
        // the component answers an abort with an error, never a suspension;
        // servicing one would let a compromised worker call the host past
        // the budget, so it ends the worker instead
        if (turn.kind !== 'error' && turn.kind !== 'crashed') {
          this.dead = true
          turn = { kind: 'protocol', message: `worker answered abort-feed with ${turn.kind}` }
        }
      }
    }
    if (turn.kind === 'crashed') this.dead = true
    return turn
  }

  /** Sends a control request and verifies its expected event kind. */
  private async control(request: ComponentRequest, kind: ComponentEvent['tag'], what: string): Promise<ComponentEvent> {
    const event = await this.run(request, undefined)
    if (!event) throw new Error(`${what} produced no turn-ending event (worker crashed)`)
    if (event.tag !== kind) throw new Error(`${what} expected event ${kind}, got ${event.tag}`)
    return event
  }

  /** Runs one turn, forwarding buffered prints and retaining its terminator. */
  private async run(request: ComponentRequest, onPrint: OnPrint | undefined): Promise<ComponentEvent | null> {
    let events: ComponentEvent[]
    try {
      const result = await this.dispatcher(request)
      if (result.status === 'shutdown') this.dead = true
      // the component reports the limit in force (the configured one, else
      // the 1000 default; a dump's on load), so it is adopted from the reply
      if (request.tag === 'configure' || request.tag === 'load') {
        this.suspensionLimit = result.maxSuspensions
        this.suspensionsSeen = 0n
      }
      events = result.events
    } catch {
      return null
    }
    let terminating: ComponentEvent | null = null
    for (const event of events) {
      if (event.tag === 'print') {
        onPrint?.(event.val.stderr ? 'stderr' : 'stdout', event.val.text)
      } else {
        terminating = event
      }
    }
    return terminating
  }

  /** Projects one semantic component event into `MontySession`'s turn shape. */
  private toTurn(event: ComponentEvent): NativeTurn {
    switch (event.tag) {
      case 'complete':
        return { kind: 'complete', value: decodeArena(event.val.values)(event.val.value) }
      case 'error':
        return { kind: 'error', exception: event.val }
      case 'typing-error':
        return { kind: 'typingError', diagnostics: event.val }
      case 'function-call': {
        this.pendingCallId = event.val.callId
        this.pendingFunctionName = event.val.functionName
        // one decode per call, so an object passed twice is one host object
        const get = decodeArena(event.val.values)
        return {
          kind: 'functionCall',
          functionName: event.val.functionName,
          args: Array.from(event.val.args, get),
          kwargs: event.val.kwargs.map(({ key, value }) => [get(key), get(value)]),
          callId: event.val.callId,
          // null (not undefined) for plain calls, matching the napi turn shape
          objectId: event.val.objectId ?? null,
          allowEagerAwait: event.val.allowEagerAwait,
        }
      }
      case 'os-call': {
        this.pendingCallId = event.val.callId
        this.pendingFunctionName = event.val.functionName
        const get = decodeArena(event.val.values)
        return {
          kind: 'osCall',
          functionName: event.val.functionName,
          args: Array.from(event.val.args, get),
          kwargs: event.val.kwargs.map(({ key, value }) => [get(key), get(value)]),
          callId: event.val.callId,
          allowEagerAwait: event.val.allowEagerAwait,
        }
      }
      case 'name-lookup':
        return { kind: 'nameLookup', name: event.val.name, objectId: event.val.objectId ?? null }
      case 'resolve-futures':
        return { kind: 'resolveFutures', pendingCallIds: [...event.val] }
      case 'fatal-error':
        return crashed(event.val)
      default:
        return { kind: 'protocol', message: `unexpected event kind ${event.tag}` }
    }
  }
}

/** Maps the public diagnostic name to the component's WIT enum. */
function componentTypeCheckFormat(format: TypeCheckFormat): ComponentTypeCheckFormat {
  const formats = ['full', 'concise', 'azure', 'json', 'json-lines', 'rdjson', 'pylint', 'gitlab', 'github'] as const
  return formats[encodeTypeCheckFormat(format) - 1]
}

/**
 * Maps the normalized options onto the WIT `auto-os-calls` record, or
 * `undefined` when every field is the worker's default.
 */
function componentAutoOsCalls(calls: EncodedAutoOsCalls): ComponentAutoOsCalls | undefined {
  const record: ComponentAutoOsCalls = {}
  if (calls.datetime === 'system') record.datetime = { tag: 'system' }
  else if (calls.datetime === 'call_host') record.datetime = { tag: 'call-host' }
  else if (calls.datetime !== undefined) record.datetime = { tag: 'fixed', val: calls.datetime }
  if (calls.timezone !== undefined) record.timezone = componentTimeZone(calls.timezone)
  if (calls.sleep === 'system' || (calls.sleep === undefined && calls.sleepSystemMaxSecs !== undefined)) {
    // the maximum only applies to a system sleep; u64::MAX lifts it, as the
    // native binding's `Duration::MAX` does
    const max = calls.sleepSystemMaxSecs
    const val =
      max === undefined ? undefined : max === Infinity ? 0xffff_ffff_ffff_ffffn : BigInt(Math.round(max * 1_000_000))
    record.sleep = { tag: 'system', val }
  } else if (calls.sleep === 'call_host') record.sleep = { tag: 'call-host' }
  else if (calls.sleep === 'zero') record.sleep = { tag: 'zero' }
  if (calls.randomStart === 'call_host') record.randomStart = { tag: 'call-host' }
  else if (calls.randomStart !== undefined) {
    record.randomStart = { tag: 'seed', val: componentRandomSeed(calls.randomStart.seed) }
  }
  return Object.keys(record).length === 0 ? undefined : record
}

/** The zone as the WIT `time-zone` variant. */
function componentTimeZone(timezone: NonNullable<EncodedAutoOsCalls['timezone']>): ComponentTimeZone {
  if (timezone === 'system') return { tag: 'system' }
  if (timezone === 'call_host') return { tag: 'call-host' }
  return { tag: 'fixed', val: { offsetSeconds: timezone.offsetSeconds, name: timezone.name } }
}

/** The seed as the WIT `random-seed` variant. */
function componentRandomSeed(seed: EncodedRandomSeed): ComponentRandomSeed {
  if ('int' in seed) return { tag: 'int', val: seed.int }
  if ('float' in seed) return { tag: 'float', val: seed.float }
  if ('str' in seed) return { tag: 'str', val: seed.str }
  return { tag: 'bytes', val: seed.bytes }
}

/** Converts JavaScript-facing limits to canonical WIT integer fields. */
function encodeLimits(limits: ResourceLimits): ComponentResourceLimits {
  return {
    ...micros('maxFeedDurationMicros', 'maxFeedDurationSecs', limits.maxFeedDurationSecs),
    ...micros('maxTurnDurationMicros', 'maxTurnDurationSecs', limits.maxTurnDurationSecs),
    ...(limits.maxMemory === undefined ? {} : { maxMemoryBytes: BigInt(limits.maxMemory) }),
    ...(limits.gcInterval === undefined ? {} : { gcInterval: BigInt(limits.gcInterval) }),
    ...(limits.maxRecursionDepth === undefined ? {} : { maxRecursionDepth: BigInt(limits.maxRecursionDepth) }),
    ...(limits.maxSuspensions === undefined ? {} : { maxSuspensions: BigInt(limits.maxSuspensions) }),
  }
}

/**
 * Renders one optional duration limit as its canonical WIT microsecond field.
 *
 * The WIT field is a `u64`, so a negative or non-finite value would either
 * throw an opaque `RangeError` out of `BigInt` or encode as a nonsense budget.
 * Reject it here instead, as the napi pool's `js_number_to_duration` does.
 *
 * `key` is the field union rather than `string`: every WIT limit is optional,
 * so a misspelled key would be dropped silently instead of failing to compile.
 */
function micros(
  key: 'maxFeedDurationMicros' | 'maxTurnDurationMicros',
  option: string,
  seconds: number | undefined,
): Partial<ComponentResourceLimits> {
  if (seconds === undefined) {
    return {}
  }
  if (!Number.isFinite(seconds) || seconds < 0) {
    throw new TypeError(`invalid ${option}: expected a non-negative number of seconds, got ${seconds}`)
  }
  return { [key]: BigInt(Math.round(seconds * 1_000_000)) }
}

/** Identifies turns that consume the host-side suspension budget. */
function isSuspension(turn: NativeTurn): boolean {
  return (
    turn.kind === 'functionCall' ||
    turn.kind === 'osCall' ||
    turn.kind === 'nameLookup' ||
    turn.kind === 'resolveFutures'
  )
}

/** Converts a host return value and its arena, turning conversion failures
 *  into Python `TypeError`. */
function returnValue(value: unknown): [CallResult, Arena] {
  try {
    const encoder = new ArenaEncoder()
    const root = encoder.push(value)
    return [{ tag: 'return-value', val: root }, encoder.finish()]
  } catch (error) {
    return [errorResult('TypeError', error instanceof Error ? error.message : String(error)), EMPTY_ARENA]
  }
}

/**
 * Resolves a feed's working directory the way `monty-pool` does for native
 * workers: unset keeps the session's current directory (the root until a
 * feed or `os.chdir` changes it — there are no mounts in the browser to
 * default to), an explicit value must be an absolute POSIX path without NUL
 * bytes and loses its trailing slashes. A rejected value is the
 * session-preserving `ValueError` turn the native path produces, so keep
 * this in step with `validate_cwd` in `monty-types` (`mount.spec.ts` runs
 * the same rejected values through both backends).
 */
function feedCwd(cwd: string | undefined): string | NativeTurn {
  const invalid = (problem: string): NativeTurn => ({
    kind: 'error',
    exception: {
      excType: 'ValueError',
      message: `cwd ${problem}: ${rustDebugString(cwd ?? '')}`,
      traceback: '',
      frames: [],
    },
  })
  if (cwd === undefined) {
    return ''
  }
  if (cwd.includes('\0')) {
    return invalid('must not contain NUL bytes')
  }
  if (!cwd.startsWith('/')) {
    return invalid('must be an absolute POSIX path')
  }
  const trimmed = cwd.replace(/\/+$/, '')
  return trimmed === '' ? '/' : trimmed
}

/**
 * Quotes a string the way Rust's `{:?}` does for ASCII (`\0`, `\n`, `\t`,
 * `\r`, `\\`, `\"`, other control characters as `\u{xx}`), so a wasm-side
 * `ValueError` matches the native one byte for byte. Non-ASCII passes through,
 * which Rust also does for printable characters.
 */
function rustDebugString(value: string): string {
  const escapes: Record<string, string> = {
    '\0': '\\0',
    '\n': '\\n',
    '\t': '\\t',
    '\r': '\\r',
    '\\': '\\\\',
    '"': '\\"',
  }
  // oxlint-disable-next-line no-control-regex
  const quoted = value.replace(/[\0\n\t\r\\"\x01-\x1f\x7f]/g, (char) => {
    return escapes[char] ?? `\\u{${char.charCodeAt(0).toString(16)}}`
  })
  return `"${quoted}"`
}

/** Creates a traceback-free host exception result. */
function errorResult(excType: string, message: string): CallResult {
  return { tag: 'error', val: { excType, message } }
}

/** Converts a lazy attribute's value, turning conversion failures into Python `TypeError`. */
function lazyAttrValue(value: unknown): NameLookupRequest {
  try {
    const encoder = new ArenaEncoder()
    const root = encoder.push(value)
    return { outcome: { tag: 'value', val: root }, values: encoder.finish() }
  } catch (error) {
    return {
      outcome: {
        tag: 'error',
        val: { excType: 'TypeError', message: error instanceof Error ? error.message : String(error) },
      },
      values: EMPTY_ARENA,
    }
  }
}

/** Creates the standard worker-crash turn. */
function crashed(message: string): NativeTurn {
  return { kind: 'crashed', message, timedOut: false }
}
