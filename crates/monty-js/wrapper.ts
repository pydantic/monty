// Custom error classes that extend Error for proper JavaScript error handling.
// These wrap the native Rust classes to provide instanceof support.

import type {
  ExceptionInfo,
  ExceptionInput,
  Frame,
  JsMontyObject,
  MontyOptions,
  ResourceLimits,
  ResumeOptions,
  RunOptions,
  SnapshotLoadOptions,
  StartOptions,
} from './index.js'

import {
  Monty as NativeMonty,
  MontyRepl as NativeMontyRepl,
  MontySnapshot as NativeMontySnapshot,
  MontyComplete as NativeMontyComplete,
  MontyException as NativeMontyException,
  MontyTypingError as NativeMontyTypingError,
} from './index.js'

export type {
  MontyOptions,
  RunOptions,
  ResourceLimits,
  Frame,
  ExceptionInfo,
  StartOptions,
  ResumeOptions,
  ExceptionInput,
  SnapshotLoadOptions,
  JsMontyObject,
}

/**
 * Alias for ResourceLimits (deprecated name).
 */
export type JsResourceLimits = ResourceLimits

/**
 * Base class for all Monty interpreter errors.
 *
 * This is the parent class for `MontySyntaxError`, `MontyRuntimeError`, and `MontyTypingError`.
 * Catching `MontyError` will catch any exception raised by Monty.
 */
export class MontyError extends Error {
  protected _typeName: string
  protected _message: string

  constructor(typeName: string, message: string) {
    super(message ? `${typeName}: ${message}` : typeName)
    this.name = 'MontyError'
    this._typeName = typeName
    this._message = message
    // Maintains proper stack trace for where our error was thrown (only available on V8)
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, MontyError)
    }
  }

  /**
   * Returns information about the inner Python exception.
   */
  get exception(): ExceptionInfo {
    return {
      typeName: this._typeName,
      message: this._message,
    }
  }

  /**
   * Returns formatted exception string.
   * @param format - 'type-msg' for 'ExceptionType: message', 'msg' for just the message
   */
  display(format: 'type-msg' | 'msg' = 'msg'): string {
    switch (format) {
      case 'msg':
        return this._message
      case 'type-msg':
        return this._message ? `${this._typeName}: ${this._message}` : this._typeName
      default:
        throw new Error(`Invalid display format: '${format}'. Expected 'type-msg' or 'msg'`)
    }
  }
}

/**
 * Raised when Python code has syntax errors or cannot be parsed by Monty.
 *
 * The inner exception is always a `SyntaxError`. Use `display()` to get
 * formatted error output.
 */
export class MontySyntaxError extends MontyError {
  private _native: NativeMontyException | null

  constructor(messageOrNative: string | NativeMontyException) {
    if (typeof messageOrNative === 'string') {
      super('SyntaxError', messageOrNative)
      this._native = null
    } else {
      const exc = messageOrNative.exception
      super('SyntaxError', exc.message)
      this._native = messageOrNative
    }
    this.name = 'MontySyntaxError'
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, MontySyntaxError)
    }
  }

  /**
   * Returns formatted exception string.
   * @param format - 'type-msg' for 'SyntaxError: message', 'msg' for just the message
   */
  override display(format: 'type-msg' | 'msg' = 'msg'): string {
    if (this._native && typeof this._native.display === 'function') {
      return this._native.display(format)
    }
    return super.display(format)
  }
}

/**
 * Raised when Monty code fails during execution.
 *
 * Provides access to the traceback frames where the error occurred via `traceback()`,
 * and formatted output via `display()`.
 */
export class MontyRuntimeError extends MontyError {
  private _native: NativeMontyException | null
  private _tracebackString: string | null
  private _frames: Frame[] | null

  constructor(
    nativeOrTypeName: NativeMontyException | string,
    message?: string,
    tracebackString?: string,
    frames?: Frame[],
  ) {
    if (typeof nativeOrTypeName === 'string') {
      // Legacy constructor: (typeName, message, tracebackString, frames)
      super(nativeOrTypeName, message!)
      this._native = null
      this._tracebackString = tracebackString ?? null
      this._frames = frames ?? null
    } else {
      // New constructor: (nativeException)
      const exc = nativeOrTypeName.exception
      super(exc.typeName, exc.message)
      this._native = nativeOrTypeName
      this._tracebackString = null
      this._frames = null
    }
    this.name = 'MontyRuntimeError'
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, MontyRuntimeError)
    }
  }

  /**
   * Returns the Monty traceback as an array of Frame objects.
   */
  traceback(): Frame[] {
    if (this._native) {
      return this._native.traceback()
    }
    return this._frames || []
  }

  /**
   * Returns formatted exception string.
   * @param format - 'traceback' for full traceback, 'type-msg' for 'ExceptionType: message', 'msg' for just the message
   */
  display(format: 'traceback' | 'type-msg' | 'msg' = 'traceback'): string {
    if (this._native && typeof this._native.display === 'function') {
      return this._native.display(format)
    }
    // Fallback for legacy constructor
    switch (format) {
      case 'traceback':
        return this._tracebackString || this.message
      case 'type-msg':
        return this._message ? `${this._typeName}: ${this._message}` : this._typeName
      case 'msg':
        return this._message
      default:
        throw new Error(`Invalid display format: '${format}'. Expected 'traceback', 'type-msg', or 'msg'`)
    }
  }
}

export type TypingDisplayFormat =
  | 'full'
  | 'concise'
  | 'azure'
  | 'json'
  | 'jsonlines'
  | 'rdjson'
  | 'pylint'
  | 'gitlab'
  | 'github'

/**
 * Raised when type checking finds errors in the code.
 *
 * This exception is raised when static type analysis detects type errors.
 * Use `displayDiagnostics()` to render rich diagnostics in various formats for tooling integration.
 * Use `display()` (inherited) for simple 'type-msg' or 'msg' formats.
 */
export class MontyTypingError extends MontyError {
  private _native: NativeMontyTypingError | null

  constructor(messageOrNative: string | NativeMontyTypingError, nativeError: NativeMontyTypingError | null = null) {
    if (typeof messageOrNative === 'string') {
      super('TypeError', messageOrNative)
      this._native = nativeError
    } else {
      const exc = messageOrNative.exception
      super('TypeError', exc.message)
      this._native = messageOrNative
    }
    this.name = 'MontyTypingError'
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, MontyTypingError)
    }
  }

  /**
   * Renders rich type error diagnostics for tooling integration.
   *
   * @param format - Output format (default: 'full')
   * @param color - Include ANSI color codes (default: false)
   */
  displayDiagnostics(format: TypingDisplayFormat = 'full', color: boolean = false): string {
    if (this._native && typeof this._native.display === 'function') {
      return this._native.display(format, color)
    }
    return this._message
  }
}

/**
 * Wrapped Monty class that throws proper Error subclasses.
 */
export class Monty {
  private _native: NativeMonty

  /**
   * Creates a new Monty interpreter by parsing the given code.
   *
   * @param code - Python code to execute
   * @param options - Configuration options
   * @throws {MontySyntaxError} If the code has syntax errors
   * @throws {MontyTypingError} If type checking is enabled and finds errors
   */
  constructor(code: string, options?: MontyOptions) {
    const result = NativeMonty.create(code, options)

    if (result instanceof NativeMontyException) {
      // Check typeName to distinguish syntax errors from other exceptions
      if (result.exception.typeName === 'SyntaxError') {
        throw new MontySyntaxError(result)
      }
      throw new MontyRuntimeError(result)
    }
    if (result instanceof NativeMontyTypingError) {
      throw new MontyTypingError(result)
    }

    this._native = result
  }

  /**
   * Performs static type checking on the code.
   *
   * @param prefixCode - Optional code to prepend before type checking
   * @throws {MontyTypingError} If type checking finds errors
   */
  typeCheck(prefixCode?: string): void {
    const result = this._native.typeCheck(prefixCode)
    if (result instanceof NativeMontyTypingError) {
      throw new MontyTypingError(result)
    }
  }

  /**
   * Executes the code and returns the result.
   *
   * @param options - Execution options (inputs, limits)
   * @returns The result of the last expression
   * @throws {MontyRuntimeError} If the code raises an exception
   */
  run(options?: RunOptions): JsMontyObject {
    const result = this._native.run(options)
    if (result instanceof NativeMontyException) {
      throw new MontyRuntimeError(result)
    }
    return result
  }

  /**
   * Starts execution and returns either a snapshot (paused at external call) or completion.
   *
   * @param options - Execution options (inputs, limits)
   * @returns MontySnapshot if an external function call is pending, MontyComplete if done
   * @throws {MontyRuntimeError} If the code raises an exception
   */
  start(options?: StartOptions): MontySnapshot | MontyComplete {
    const result = this._native.start(options)
    return wrapStartResult(result)
  }

  /**
   * Serializes the Monty instance to a binary format.
   */
  dump(): Buffer {
    return this._native.dump()
  }

  /**
   * Deserializes a Monty instance from binary format.
   */
  static load(data: Buffer): Monty {
    const instance = Object.create(Monty.prototype) as Monty
    instance._native = NativeMonty.load(data)
    return instance
  }

  /** Returns the script name. */
  get scriptName(): string {
    return this._native.scriptName
  }

  /** Returns the input variable names. */
  get inputs(): string[] {
    return this._native.inputs
  }

  /** Returns the external function names. */
  get externalFunctions(): string[] {
    return this._native.externalFunctions
  }

  /** Returns a string representation of the Monty instance. */
  repr(): string {
    return this._native.repr()
  }
}

/**
 * Incremental no-replay REPL session.
 */
export class MontyRepl {
  private _native: NativeMontyRepl

  /**
   * Creates a REPL session directly from source code.
   */
  static create(code: string, options?: MontyOptions, startOptions?: StartOptions): MontyRepl {
    const result = NativeMontyRepl.create(code, options, startOptions)
    if (result instanceof NativeMontyException) {
      if (result.exception.typeName === 'SyntaxError') {
        throw new MontySyntaxError(result)
      }
      throw new MontyRuntimeError(result)
    }
    if (result instanceof NativeMontyTypingError) {
      throw new MontyTypingError(result)
    }
    return new MontyRepl(result)
  }

  constructor(nativeRepl: NativeMontyRepl) {
    this._native = nativeRepl
  }

  /** Returns the script name for this REPL session. */
  get scriptName(): string {
    return this._native.scriptName
  }

  /**
   * Executes one incremental snippet.
   *
   * @param code - Snippet code to execute
   * @returns Snippet output
   * @throws {MontyRuntimeError} If execution raises an exception
   */
  feed(code: string): JsMontyObject {
    const result = this._native.feed(code)
    if (result instanceof NativeMontyException) {
      throw new MontyRuntimeError(result)
    }
    return result
  }

  /** Serializes the REPL session to bytes. */
  dump(): Buffer {
    return this._native.dump()
  }

  /** Restores a REPL session from bytes. */
  static load(data: Buffer): MontyRepl {
    return new MontyRepl(NativeMontyRepl.load(data))
  }

  /** Returns a string representation of the REPL session. */
  repr(): string {
    return this._native.repr()
  }
}

/**
 * Helper to wrap native start/resume results, throwing errors as needed.
 */
function wrapStartResult(
  result: NativeMontySnapshot | NativeMontyComplete | NativeMontyException,
): MontySnapshot | MontyComplete {
  if (result instanceof NativeMontyException) {
    throw new MontyRuntimeError(result)
  }
  if (result instanceof NativeMontySnapshot) {
    return new MontySnapshot(result)
  }
  if (result instanceof NativeMontyComplete) {
    return new MontyComplete(result)
  }
  throw new Error(`Unexpected result type from native binding: ${result}`)
}

/**
 * Represents paused execution waiting for an external function call return value.
 *
 * Contains information about the pending external function call and allows
 * resuming execution with the return value or an exception.
 */
export class MontySnapshot {
  private _native: NativeMontySnapshot

  constructor(nativeSnapshot: NativeMontySnapshot) {
    this._native = nativeSnapshot
  }

  /** Returns the name of the script being executed. */
  get scriptName(): string {
    return this._native.scriptName
  }

  /** Returns the name of the external function being called. */
  get functionName(): string {
    return this._native.functionName
  }

  /** Returns the positional arguments passed to the external function. */
  get args(): JsMontyObject[] {
    return this._native.args
  }

  /** Returns the keyword arguments passed to the external function as an object. */
  get kwargs(): Record<string, JsMontyObject> {
    return this._native.kwargs as Record<string, JsMontyObject>
  }

  /**
   * Resumes execution with either a return value or an exception.
   *
   * @param options - Object with either `returnValue` or `exception`
   * @returns MontySnapshot if another external call is pending, MontyComplete if done
   * @throws {MontyRuntimeError} If the code raises an exception
   */
  resume(options: ResumeOptions): MontySnapshot | MontyComplete {
    const result = this._native.resume(options)
    return wrapStartResult(result)
  }

  /**
   * Serializes the MontySnapshot to a binary format.
   */
  dump(): Buffer {
    return this._native.dump()
  }

  /**
   * Deserializes a MontySnapshot from binary format.
   */
  static load(data: Buffer, options?: SnapshotLoadOptions): MontySnapshot {
    const nativeSnapshot = NativeMontySnapshot.load(data, options)
    return new MontySnapshot(nativeSnapshot)
  }

  /** Returns a string representation of the MontySnapshot. */
  repr(): string {
    return this._native.repr()
  }
}

/**
 * Represents completed execution with a final output value.
 */
export class MontyComplete {
  private _native: NativeMontyComplete

  constructor(nativeComplete: NativeMontyComplete) {
    this._native = nativeComplete
  }

  /** Returns the final output value from the executed code. */
  get output(): JsMontyObject {
    return this._native.output
  }

  /** Returns a string representation of the MontyComplete. */
  repr(): string {
    return this._native.repr()
  }
}

/**
 * Options for `runMontyAsync`.
 */
export interface RunMontyAsyncOptions {
  /** Input values for the script. */
  inputs?: Record<string, JsMontyObject>
  /** External function implementations (sync or async). */
  externalFunctions?: Record<string, (...args: unknown[]) => unknown>
  /** Resource limits. */
  limits?: ResourceLimits
}

/**
 * Runs a Monty script with async external function support.
 *
 * This function handles both synchronous and asynchronous external functions.
 * When an external function returns a Promise, it will be awaited before
 * resuming execution.
 *
 * @param montyRunner - The Monty runner instance to execute
 * @param options - Execution options
 * @returns The output of the Monty script
 * @throws {MontyRuntimeError} If the code raises an exception
 * @throws {MontySyntaxError} If the code has syntax errors
 *
 * @example
 * const m = new Monty('result = await fetch_data(url)', {
 *   inputs: ['url'],
 *   externalFunctions: ['fetch_data']
 * });
 *
 * const result = await runMontyAsync(m, {
 *   inputs: { url: 'https://example.com' },
 *   externalFunctions: {
 *     fetch_data: async (url) => {
 *       const response = await fetch(url);
 *       return response.text();
 *     }
 *   }
 * });
 */
export async function runMontyAsync(montyRunner: Monty, options: RunMontyAsyncOptions = {}): Promise<JsMontyObject> {
  const { inputs, externalFunctions = {}, limits } = options

  let progress: MontySnapshot | MontyComplete = montyRunner.start({
    inputs,
    limits,
  })

  while (progress instanceof MontySnapshot) {
    const snapshot = progress
    const funcName = snapshot.functionName
    const extFunction = externalFunctions[funcName]

    if (!extFunction) {
      // Function not found - resume with a KeyError exception
      progress = snapshot.resume({
        exception: {
          type: 'KeyError',
          message: `"External function '${funcName}' not found"`,
        },
      })
      continue
    }

    try {
      // Call the external function
      let result = extFunction(...snapshot.args, snapshot.kwargs)

      // If the result is a Promise, await it
      if (result && typeof (result as Promise<unknown>).then === 'function') {
        result = await result
      }

      // Resume with the return value
      progress = snapshot.resume({ returnValue: result })
    } catch (error) {
      // External function threw an exception - convert to Monty exception
      const err = error as Error
      const excType = err.name || 'RuntimeError'
      const excMessage = err.message || String(error)
      progress = snapshot.resume({
        exception: {
          type: excType,
          message: excMessage,
        },
      })
    }
  }

  return progress.output
}
