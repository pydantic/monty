// Error classes thrown by the pool client, mirroring pydantic_monty's
// exception hierarchy: MontyError is the base, with MontySyntaxError /
// MontyRuntimeError / MontyTypingError for sandbox failures and
// MontyCrashedError for worker death. All carry the data received over the
// wire — tracebacks are rendered client-side from the proto stack frames.

import type { MontyError as PbMontyError, StackFrame as PbStackFrame } from './generated/monty/v1/monty_pb.js'

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
  private readonly frames: PbStackFrame[]

  constructor(message: string, frames: PbStackFrame[] = []) {
    super('SyntaxError', message)
    this.name = 'MontySyntaxError'
    this.frames = frames
  }

  /**
   * Formats the exception; `'traceback'` includes the source location frame
   * CPython shows for syntax errors.
   */
  override display(format: 'traceback' | 'type-msg' | 'msg' = 'msg'): string {
    if (format === 'traceback') {
      return renderTraceback(this.frames, this.typeName, this.innerMessage)
    }
    return super.display(format)
  }
}

/**
 * Raised when sandbox code fails during execution. The session survives — the
 * worker keeps its globals and later feeds still work.
 */
export class MontyRuntimeError extends MontyError {
  private readonly frames: PbStackFrame[]

  constructor(typeName: string, message: string, frames: PbStackFrame[] = []) {
    super(typeName, message)
    this.name = 'MontyRuntimeError'
    this.frames = frames
  }

  /** The Monty traceback, outermost frame first. */
  traceback(): Frame[] {
    return this.frames.map((f) => ({
      filename: f.filename,
      line: f.start?.line ?? 0,
      column: f.start?.column ?? 0,
      endLine: f.end?.line ?? 0,
      endColumn: f.end?.column ?? 0,
      ...(f.frameName !== undefined ? { functionName: f.frameName } : {}),
      ...(f.previewLine !== undefined ? { sourceLine: f.previewLine } : {}),
    }))
  }

  /**
   * Formats the exception: `'traceback'` (default) renders the full Python
   * traceback, `'type-msg'` / `'msg'` the summary forms.
   */
  override display(format: 'traceback' | 'type-msg' | 'msg' = 'traceback'): string {
    if (format === 'traceback') {
      return renderTraceback(this.frames, this.typeName, this.innerMessage)
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
 * Raised when a worker process died: it crashed hard (segfault, allocator
 * abort — the failure mode subprocess isolation exists to contain) or was
 * killed for exceeding `requestTimeout`. The session is lost; the pool
 * replaces the worker, so other sessions and future checkouts are unaffected.
 */
export class MontyCrashedError extends MontyError {
  /** True when the worker was killed by the `requestTimeout` watchdog. */
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
 * Every exception type name monty's `ExcType` can parse (the wire `exc_type`
 * is a string the worker parses; unknown names would be a protocol error).
 * Kept in lockstep with `ExcType` in crates/monty/src/exception_private.rs.
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

/** Number of identical consecutive frames shown before collapsing. */
const REPEAT_FRAMES_SHOWN = 3

/**
 * Renders a Python-format traceback from proto stack frames, matching
 * monty's `MontyException` Display implementation (which itself matches
 * CPython, except carets are always `~`).
 */
export function renderTraceback(frames: PbStackFrame[], typeName: string, message: string): string {
  let out = ''
  if (frames.length > 0) {
    out += 'Traceback (most recent call last):\n'
  }
  let i = 0
  while (i < frames.length) {
    const frame = frames[i]!
    let repeat = 1
    while (i + repeat < frames.length && framesIdentical(frame, frames[i + repeat]!)) {
      repeat += 1
    }
    const shown = repeat > REPEAT_FRAMES_SHOWN ? REPEAT_FRAMES_SHOWN : repeat
    for (let j = 0; j < shown; j++) {
      out += renderFrame(frames[i + j]!)
    }
    if (repeat > REPEAT_FRAMES_SHOWN) {
      out += `  [Previous line repeated ${repeat - REPEAT_FRAMES_SHOWN} more times]\n`
    }
    i += repeat
  }
  out += message ? `${typeName}: ${message}` : typeName
  return out
}

/** Frames collapse when filename, line, and frame name all match. */
function framesIdentical(a: PbStackFrame, b: PbStackFrame): boolean {
  return a.filename === b.filename && a.start?.line === b.start?.line && a.frameName === b.frameName
}

/** Renders one `  File "...", line N[, in name]` block with preview/carets. */
function renderFrame(f: PbStackFrame): string {
  const line = f.start?.line ?? 0
  let out = f.hideFrameName
    ? `  File "${f.filename}", line ${line}`
    : `  File "${f.filename}", line ${line}, in ${f.frameName ?? '<module>'}`

  if (f.previewLine !== undefined) {
    const trimmed = f.previewLine.replace(/^\s+/, '')
    out += `\n    ${trimmed}\n`
    if (!f.hideCaret) {
      const leadingSpaces = f.previewLine.length - trimmed.length
      const startCol = f.start?.column ?? 0
      const caretStart = startCol > leadingSpaces ? 4 + startCol - leadingSpaces - 1 : 4
      const caretLen = Math.max((f.end?.column ?? 0) - startCol, 1)
      out += `${' '.repeat(caretStart)}${'~'.repeat(caretLen)}\n`
    }
  } else {
    out += '\n'
  }
  return out
}

/**
 * Maps a wire `MontyError` to the matching error class: `SyntaxError` is a
 * parse failure, everything else a runtime exception.
 */
export function montyErrorFromProto(err: PbMontyError): MontySyntaxError | MontyRuntimeError {
  const message = err.message ?? ''
  if (err.excType === 'SyntaxError') {
    return new MontySyntaxError(message, err.traceback)
  }
  return new MontyRuntimeError(err.excType, message, err.traceback)
}
