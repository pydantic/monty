// Error classes thrown by the pool client, mirroring pydantic_monty's
// exception hierarchy: MontyError is the base, with MontySyntaxError /
// MontyRuntimeError / MontyTypingError for sandbox failures and
// MontyCrashedError for worker death. The full Python traceback is rendered
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
      return this.tracebackText || super.display('type-msg')
    }
    return super.display(format)
  }
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
 * Raised when a worker process died: a hard crash (segfault, allocator abort
 * — the failure mode subprocess isolation exists to contain) or a watchdog
 * kill for exceeding `requestTimeout` / the `maxDurationSecs` backstop. The
 * session is lost; the pool replaces the worker, so other sessions and future
 * checkouts are unaffected.
 */
export class MontyCrashedError extends MontyError {
  /** True when the worker was killed by a watchdog timeout. */
  readonly timedOut: boolean
  /** Worker exit description (e.g. `signal: 9 (SIGKILL)`), when known. */
  readonly exitStatus: string | null

  constructor(message: string, options: { timedOut?: boolean; exitStatus?: string | null } = {}) {
    super('RuntimeError', message)
    this.name = 'MontyCrashedError'
    this.timedOut = options.timedOut ?? false
    this.exitStatus = options.exitStatus ?? null
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
 * lockstep with `ExcType` in crates/monty/src/exception_private.rs.
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
  'json.JSONDecodeError',
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
    case 'object':
      if (value instanceof Uint8Array) return 'bytes'
      if (value instanceof Map) return 'dict'
      if (value instanceof Set) return 'set'
      if (Array.isArray(value)) return 'list'
      return 'dict'
    default:
      // symbols and other exotic values have no Monty equivalent
      return 'object'
  }
}
