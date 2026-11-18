// Error classes thrown by the pool client, mirroring pydantic_monty's
// exception hierarchy: MontyError is the base, with MontySyntaxError /
// MontyRuntimeError / MontyTypingError for sandbox failures and
// MontyCrashedError for worker death (MontyDisconnectError / MontyShutdown
// for a remote worker's stream). The full Python traceback is rendered
// once, in the worker (monty's `MontyException` Display), and carried across
// as a string — never re-implemented here. Structured frames travel alongside
// it for programmatic access via `MontyRuntimeError.traceback()`.

import type { NativeException, NativeFrame } from './native.js'

/** One frame of a Monty traceback. */
export interface Frame {
  filename: string
  line: number
  column: number
  endLine: number
  endColumn: number
  functionName?: string
  sourceLine?: string
}

/**
 * Where the expression that suspended execution is in the source, as
 * `snapshot.position`. `start` and `end` are UTF-8 byte offsets into the
 * source, `end` exclusive: slice `new TextEncoder().encode(source)`, not the
 * string. `filename` names the source as a traceback frame does:
 * `<python-input-N>` for the session's N-th feed, or `<string>` inside an
 * `eval()` / `exec()` string. A worker that predates the field reports none:
 * `filename` is then empty and both offsets 0.
 */
export interface SourceRange {
  filename: string
  start: number
  end: number
}

/** Inner Python exception summary. */
export interface ExceptionInfo {
  typeName: string
  message: string
}

/**
 * Base class for all Monty errors. Catching `MontyError` catches every
 * failure originating from the sandbox or its worker process.
 */
export class MontyError extends Error {
  protected readonly typeName: string
  protected readonly innerMessage: string

  constructor(typeName: string, message: string) {
    super(message ? `${typeName}: ${message}` : typeName)
    this.name = 'MontyError'
    this.typeName = typeName
    this.innerMessage = message
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, new.target)
    }
  }

  /** Information about the inner Python exception. */
  get exception(): ExceptionInfo {
    return { typeName: this.typeName, message: this.innerMessage }
  }

  /**
   * Formats the exception: `'type-msg'` for `ExceptionType: message`,
   * `'msg'` (default) for just the message.
   */
  display(format: 'type-msg' | 'msg' = 'msg'): string {
    switch (format) {
      case 'msg':
        return this.innerMessage
      case 'type-msg':
        return this.innerMessage ? `${this.typeName}: ${this.innerMessage}` : this.typeName
      default:
        throw new Error(`Invalid display format: '${format}'. Expected 'type-msg' or 'msg'`)
    }
  }
}

/**
 * Raised when the fed code cannot be parsed. The inner exception is always a
 * `SyntaxError`.
 */
export class MontySyntaxError extends MontyError {
  private readonly tracebackText: string

  constructor(message: string, tracebackText = '') {
    super('SyntaxError', message)
    this.name = 'MontySyntaxError'
    this.tracebackText = tracebackText
  }

  /**
   * Formats the exception; `'traceback'` returns the worker-rendered traceback
   * (including the source-location frame CPython shows for syntax errors),
   * falling back to the `type-msg` summary when none was supplied.
   */
  override display(format: 'traceback' | 'type-msg' | 'msg' = 'msg'): string {
    if (format === 'traceback') {
      return this.tracebackText || super.display('type-msg')
    }
    return super.display(format)
  }
}

/**
 * Raised when sandbox code fails during execution. The session survives — the
 * worker keeps its globals and later feeds still work.
 */
export class MontyRuntimeError extends MontyError {
  private readonly frames: NativeFrame[]
  private readonly tracebackText: string

  constructor(typeName: string, message: string, frames: NativeFrame[] = [], tracebackText = '') {
    super(typeName, message)
    this.name = 'MontyRuntimeError'
    this.frames = frames
    this.tracebackText = tracebackText
  }

  /** The Monty traceback, outermost frame first. */
  traceback(): Frame[] {
    return this.frames.map((f) => ({
      filename: f.filename,
      line: f.line,
      column: f.column,
      endLine: f.endLine,
      endColumn: f.endColumn,
      ...(f.frameName !== undefined ? { functionName: f.frameName } : {}),
      ...(f.previewLine !== undefined ? { sourceLine: f.previewLine } : {}),
    }))
  }

  /**
   * Formats the exception: `'traceback'` (default) returns the full Python
   * traceback rendered by the worker, `'type-msg'` / `'msg'` the summary
   * forms.
   */
  override display(format: 'traceback' | 'type-msg' | 'msg' = 'traceback'): string {
    if (format === 'traceback') {
      return this.tracebackText || renderTraceback(this.frames, super.display('type-msg'))
    }
    return super.display(format)
  }
}

function renderTraceback(frames: NativeFrame[], summary: string): string {
  if (frames.length === 0) {
    return summary
  }
  const lines = ['Traceback (most recent call last):']
  for (const frame of frames) {
    const name = frame.frameName ?? '<module>'
    lines.push(`  File "${frame.filename}", line ${frame.line}, in ${name}`)
    if (frame.previewLine !== undefined) {
      const leading = frame.previewLine.length - frame.previewLine.trimStart().length
      const preview = frame.previewLine.trimStart()
      lines.push(`    ${preview}`)
      if (!preview.startsWith('raise') && frame.column > 0 && frame.endColumn > frame.column) {
        const column = Math.max(1, frame.column - leading)
        const width = Math.max(1, frame.endColumn - frame.column)
        const indent = ' '.repeat(column - 1)
        lines.push(`    ${indent}${'~'.repeat(width)}`)
      }
    }
  }
  lines.push(summary)
  return lines.join('\n')
}

/**
 * Raised when type checking rejects a fed snippet (sessions created with
 * `typeCheck: true`). The snippet was not executed and the session survives.
 *
 * Diagnostics are rendered inside the worker; `display()` returns them
 * verbatim, one per line.
 */
export class MontyTypingError extends MontyError {
  private readonly diagnostics: string

  constructor(diagnostics: string) {
    const first = diagnostics.split('\n', 1)[0] ?? ''
    super('TypeError', first)
    this.name = 'MontyTypingError'
    this.diagnostics = diagnostics
  }

  /** The rendered type-checking diagnostics, one per line. */
  override display(): string {
    return this.diagnostics
  }
}

/**
 * Raised when a worker fails to initialize or dies: a process crash, WASM
 * trap, or watchdog kill for exceeding a request/duration deadline.
 * If a session exists, it is lost; its pool can replace the worker on future checkouts.
 */
export class MontyCrashedError extends MontyError {
  /** True when the worker was killed by a watchdog timeout. */
  readonly timedOut: boolean
  /** Worker exit description (e.g. `signal: 9 (SIGKILL)`), when known. */
  readonly exitStatus: string | null

  constructor(message: string, options: { timedOut?: boolean; exitStatus?: string | null } = {}) {
    super('RuntimeError', message)
    this.message = message
    this.name = 'MontyCrashedError'
    this.timedOut = options.timedOut ?? false
    this.exitStatus = options.exitStatus ?? null
  }
}

/** The status or transport failure that ended a remote session's stream. */
export interface DisconnectDetails {
  /**
   * The gRPC status code the server ended the stream with, by its canonical
   * name (`DEADLINE_EXCEEDED`, `RESOURCE_EXHAUSTED`, ...); `OK` when it simply
   * finished the stream. Absent for a transport failure.
   */
  statusCode?: string
  /** The message on that status. */
  statusMessage?: string
  /**
   * The policy reason the server attached, in `snake_case` (`idle_timeout`,
   * `session_timeout`, `turn_timeout`, `capacity`, ...), when it is one this
   * client knows.
   */
  remoteCause?: string
  /** The transport or protocol error, when the server sent no status. */
  transportError?: string
}

/**
 * Raised when a remote worker's stream ended mid-session (`MontyRemote` only;
 * the local analogue is `MontyCrashedError`). The sandbox may have died, the
 * connection may have dropped, or the server may have ended the session by
 * policy — an idle, session or turn timeout, or being over capacity — which
 * it says in `statusCode`, `statusMessage` and `remoteCause`. None of them says
 * whether the in-flight request ran. The session is lost; check out a new one.
 */
export class MontyDisconnectError extends MontyError {
  /** The status code the server ended the stream with, by name, or `null`. */
  readonly statusCode: string | null
  /** The message on that status, or `null`. */
  readonly statusMessage: string | null
  /** The policy reason the server named, in `snake_case`, or `null`. */
  readonly remoteCause: string | null
  /** The transport or protocol error when the server sent no status, or `null`. */
  readonly transportError: string | null

  constructor(message: string, details: DisconnectDetails = {}) {
    super('RuntimeError', message)
    this.message = message
    this.name = 'MontyDisconnectError'
    this.statusCode = details.statusCode ?? null
    this.statusMessage = details.statusMessage ?? null
    this.remoteCause = details.remoteCause ?? null
    this.transportError = details.transportError ?? null
  }
}

/** The status a server refused a session with. */
export interface RefusalDetails {
  /** The gRPC status code, by its canonical name (`RESOURCE_EXHAUSTED`, ...). */
  statusCode?: string
  /** The message on that status. */
  statusMessage?: string
  /** The policy reason the server named, in `snake_case`. */
  remoteCause?: string
  /** Suggested retry delay in seconds, parsed from `Retry-After`. */
  retryAfter?: number
  /** Whether the server returned `RESOURCE_EXHAUSTED` or `UNAVAILABLE`. */
  retryable?: boolean
}

/**
 * Raised when the remote server refused to open a session's stream
 * (`MontyRemote` only): at capacity (`capacity`, or `caller_cap` for this
 * caller's share), with no worker to serve it (`worker_unavailable`), or
 * rejecting the caller's credentials (`unauthenticated`) or what a valid
 * credential asked for (`permission_denied`), or returning `FAILED_PRECONDITION`.
 * The server gives details in `statusCode`,
 * `statusMessage` and `remoteCause`. No session existed, so
 * nothing ran. `retryable` identifies temporary refusals; `retryAfter` is
 * the server's suggested delay, not an automatic retry. A server that
 * cannot be reached, that answers with any other status (such as
 * `UNIMPLEMENTED` from a URL that is no Monty server), or that is no gRPC
 * server at all (a web server's error page), rejects with `MontyConnectError`
 * instead.
 */
export class MontyRefusedError extends MontyError {
  /** The status code the server refused with, by name, or `null`. */
  readonly statusCode: string | null
  /** The message on that status, or `null`. */
  readonly statusMessage: string | null
  /** The policy reason the server named, in `snake_case`, or `null`. */
  readonly remoteCause: string | null
  /** True for `RESOURCE_EXHAUSTED` or `UNAVAILABLE`; credentials must be fixed separately. */
  readonly retryable: boolean
  /** Suggested delay in seconds, or `null` for absent or invalid `Retry-After`. */
  readonly retryAfter: number | null

  constructor(message: string, details: RefusalDetails = {}) {
    super('RuntimeError', message)
    this.message = message
    this.name = 'MontyRefusedError'
    this.statusCode = details.statusCode ?? null
    this.statusMessage = details.statusMessage ?? null
    this.remoteCause = details.remoteCause ?? null
    this.retryable = details.retryable ?? ['RESOURCE_EXHAUSTED', 'UNAVAILABLE'].includes(this.statusCode ?? '')
    this.retryAfter = details.retryAfter ?? null
  }
}

/** Raised when a remote connection fails before a session opens. */
export class MontyConnectError extends MontyError {
  /** The gRPC status code, or `null` without a server status. */
  readonly statusCode: string | null
  /** The server's status message, or `null`. */
  readonly statusMessage: string | null
  /** The policy reason the server named, in `snake_case`, or `null`. */
  readonly remoteCause: string | null
  /** The underlying transport error, or `null` when the server sent a status. */
  readonly transportError: string | null

  constructor(message: string, details: DisconnectDetails = {}) {
    super('RuntimeError', message)
    this.message = message
    this.name = 'MontyConnectError'
    this.statusCode = details.statusCode ?? null
    this.statusMessage = details.statusMessage ?? null
    this.remoteCause = details.remoteCause ?? null
    this.transportError = details.transportError ?? null
  }
}

/**
 * Raised for invalid pool settings, malformed request metadata, or settings incompatible with the remote server.
 * A worker or server refusing the session's configuration (`session refused: …`) raises it from the session's first
 * request, which did not run, or from `getSessionId()` when it opens the session alone, and the session is lost;
 * the WASM worker raises it from `checkout()`.
 */
export class MontyConfigurationError extends MontyError {
  constructor(message: string) {
    super('ValueError', message)
    this.message = message
    this.name = 'MontyConfigurationError'
  }
}

/**
 * Raised when the remote server is shutting down (`MontyRemote` only). The
 * request that raised it **did not run**, so re-running it is safe; the
 * session is lost. Against a server that stores sessions the pool resumes the
 * session itself first (see `MontyRemoteOptions.autoResume`), so this is
 * raised only when that fails or is disabled.
 *
 * `dump` restores the session on a fresh one, with `loadSession` (idle) or
 * `loadSnapshot` (suspended mid-feed); `null` when the server stored nothing.
 * Restoring a suspended call re-announces it, so a host callback the
 * interrupted request was answering runs again.
 */
export class MontyShutdown extends MontyError {
  /** What `loadSession` / `loadSnapshot` restores, or `null` when there is nothing to load. */
  readonly dump: Buffer | null

  constructor(message: string, dump: Buffer | null = null) {
    super('RuntimeError', message)
    this.message = message
    this.name = 'MontyShutdown'
    this.dump = dump
  }
}

/**
 * Raised when the worker (or a caller misusing the session) violated the
 * wire protocol. The worker has been discarded; the session is lost.
 */
export class ProtocolError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'ProtocolError'
  }
}

/**
 * Every exception type name monty's `ExcType` can parse (the native binding
 * parses the name; unknown names fall back to `RuntimeError`). Kept in
 * lockstep with `ExcType` in crates/monty-types/src/exceptions.rs.
 */
export const PYTHON_EXC_NAMES: ReadonlySet<string> = new Set([
  'Exception',
  'BaseException',
  'SystemExit',
  'KeyboardInterrupt',
  'ArithmeticError',
  'OverflowError',
  'ZeroDivisionError',
  'LookupError',
  'IndexError',
  'KeyError',
  'RuntimeError',
  'NotImplementedError',
  'RecursionError',
  'AttributeError',
  'FrozenInstanceError',
  'NameError',
  'UnboundLocalError',
  'ValueError',
  'UnicodeDecodeError',
  'UnicodeEncodeError',
  'json.JSONDecodeError',
  'tomllib.TOMLDecodeError',
  'ImportError',
  'ModuleNotFoundError',
  'OSError',
  'FileNotFoundError',
  'FileExistsError',
  'IsADirectoryError',
  'NotADirectoryError',
  'PermissionError',
  'io.UnsupportedOperation',
  'AssertionError',
  'MemoryError',
  'StopIteration',
  'SyntaxError',
  'TimeoutError',
  'TypeError',
  're.PatternError',
  'binascii.Error',
  'binascii.Incomplete',
  'statistics.StatisticsError',
  '_csv.Error',
  'decimal.DecimalException',
  'decimal.Clamped',
  'decimal.InvalidOperation',
  'decimal.ConversionSyntax',
  'decimal.DivisionByZero',
  'decimal.DivisionImpossible',
  'decimal.DivisionUndefined',
  'decimal.Inexact',
  'decimal.InvalidContext',
  'decimal.Rounded',
  'decimal.Subnormal',
  'decimal.Overflow',
  'decimal.Underflow',
  'decimal.FloatOperation',
  'pydantic_core._pydantic_core.ValidationError',
  'pydantic_core._pydantic_core.PydanticSerializationError',
  'pydantic.errors.PydanticUserError',
  'pydantic.errors.PydanticSchemaGenerationError',
])

/**
 * Maps a native exception to the matching error class: `SyntaxError` is a
 * parse failure, everything else a runtime exception.
 */
export function montyErrorFromNative(exc: NativeException): MontySyntaxError | MontyRuntimeError {
  if (exc.excType === 'SyntaxError') {
    return new MontySyntaxError(exc.message, exc.traceback)
  }
  return new MontyRuntimeError(exc.excType, exc.message, exc.frames, exc.traceback)
}

/**
 * CPython-style `TypeError` message for calling a non-callable
 * `externalLookup` entry — reachable when a cached function proxy's entry is
 * later replaced by a plain value. Mirrors what CPython raises when calling
 * that value, matching the Python binding (which really calls the entry).
 */
export function notCallableMessage(value: unknown): string {
  return `'${pyTypeName(value)}' object is not callable`
}

/**
 * `__monty_type__` marker → the Python type its conversion produces. A
 * `ClassInstance` marker is named by its class (see [`pyTypeName`]); an
 * unknown marker converts as a plain dict.
 */
const MARKED_TYPE_NAMES: Readonly<Record<string, string>> = {
  Ellipsis: 'ellipsis',
  Exception: 'Exception',
  Date: 'date',
  DateTime: 'datetime',
  Time: 'time',
  TimeDelta: 'timedelta',
  TimeZone: 'timezone',
  Type: 'type',
  BuiltinFunction: 'builtin_function_or_method',
}

/** Python type name the JS value converts to (mirrors the Rust `js_to_monty`). */
function pyTypeName(value: unknown): string {
  if (value === null || value === undefined) {
    return 'NoneType'
  }
  switch (typeof value) {
    case 'boolean':
      return 'bool'
    case 'number':
      return Number.isInteger(value) ? 'int' : 'float'
    case 'bigint':
      return 'int'
    case 'string':
      return 'str'
    case 'function':
      return 'function'
    case 'object': {
      if (value instanceof Uint8Array) return 'bytes'
      if (value instanceof Map) return 'dict'
      if (value instanceof Set) return 'set'
      if (Array.isArray(value)) {
        return readMarker(value, '__tuple__') ? 'tuple' : 'list'
      }
      const marker = readMarker(value, '__monty_type__')
      if (marker === 'ClassInstance') {
        // named by its class, as `type(x).__name__` is for the converted instance
        const classType = readMarker(value, 'type')
        const name = typeof classType === 'object' && classType !== null ? readMarker(classType, 'name') : undefined
        return typeof name === 'string' ? name : 'dict'
      }
      return typeof marker === 'string' ? (MARKED_TYPE_NAMES[marker] ?? 'dict') : 'dict'
    }
    default:
      // symbols and other exotic values have no Monty equivalent
      return 'object'
  }
}

/**
 * Reads a marker property off a host-provided value without letting a throwing
 * getter or `Proxy` trap escape. `pyTypeName` runs *while formatting a
 * TypeError message*, and the drive loop treats any throw from a call handler as
 * fatal (it marks the session broken), so an exotic `externalLookup` entry must
 * still degrade to a plain type rather than poison the turn — mirroring the Rust
 * `js_to_monty`, which falls back to `object`/`dict` on any conversion failure.
 */
function readMarker(value: object, key: string): unknown {
  try {
    return (value as Record<string, unknown>)[key]
  } catch {
    return undefined
  }
}
