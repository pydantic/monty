/**
 * Monty - A sandboxed Python interpreter for JavaScript/TypeScript.
 *
 * This module provides proper Error subclasses for exception handling.
 */

import type {
  MontyOptions,
  RunOptions,
  ResourceLimits,
  Frame,
  ExceptionInfo,
  RuntimeErrorInfo,
  JsMontyObject,
  StartOptions,
  ResumeOptions,
  ExceptionInput,
  SnapshotLoadOptions,
} from './index'

export type {
  MontyOptions,
  RunOptions,
  ResourceLimits,
  Frame,
  ExceptionInfo,
  RuntimeErrorInfo,
  JsMontyObject,
  StartOptions,
  ResumeOptions,
  ExceptionInput,
  SnapshotLoadOptions,
}

/**
 * Base class for all Monty interpreter errors.
 *
 * This is the parent class for `MontySyntaxError`, `MontyRuntimeError`, and `MontyTypingError`.
 * Catching `MontyError` will catch any exception raised by Monty.
 */
export declare class MontyError extends Error {
  constructor(typeName: string, message: string)

  /**
   * Returns information about the inner Python exception.
   */
  get exception(): ExceptionInfo

  /**
   * Returns formatted exception string.
   * @param format - 'type-msg' for 'ExceptionType: message', 'msg' for just the message
   */
  display(format?: 'type-msg' | 'msg'): string
}

/**
 * Raised when Python code has syntax errors or cannot be parsed by Monty.
 *
 * The inner exception is always a `SyntaxError`.
 */
export declare class MontySyntaxError extends MontyError {
  constructor(message: string)

  /**
   * Returns formatted exception string.
   * @param format - 'type-msg' for 'SyntaxError: message', 'msg' for just the message
   */
  display(format?: 'type-msg' | 'msg'): string
}

/**
 * Raised when Monty code fails during execution.
 *
 * Provides access to the traceback frames where the error occurred.
 */
export declare class MontyRuntimeError extends MontyError {
  constructor(typeName: string, message: string, tracebackString: string, frames: Frame[])

  /**
   * Returns the Monty traceback as an array of Frame objects.
   */
  traceback(): Frame[]

  /**
   * Returns formatted exception string.
   * @param format - 'traceback' for full traceback, 'type-msg' for 'ExceptionType: message', 'msg' for just the message
   */
  display(format?: 'traceback' | 'type-msg' | 'msg'): string
}

/**
 * Raised when type checking finds errors in the code.
 *
 * Use `display()` to render diagnostics in various formats.
 */
export declare class MontyTypingError extends MontyError {
  constructor(message: string, nativeError?: unknown)

  /**
   * Renders the type error diagnostics with the specified format and color.
   * @param format - Output format
   * @param color - Whether to include ANSI color codes
   */
  display(
    format?: 'full' | 'concise' | 'azure' | 'json' | 'jsonlines' | 'rdjson' | 'pylint' | 'gitlab' | 'github',
    color?: boolean,
  ): string
}

/**
 * A sandboxed Python interpreter instance.
 *
 * Parses and compiles Python code on initialization, then can be run
 * multiple times with different input values.
 */
export declare class Monty {
  /**
   * Creates a new Monty interpreter by parsing the given code.
   * @param code - Python code to execute
   * @param options - Configuration options
   * @throws {MontySyntaxError} If the code has syntax errors
   * @throws {MontyTypingError} If type checking is enabled and finds errors
   */
  constructor(code: string, options?: MontyOptions)

  /**
   * Performs static type checking on the code.
   * @param prefixCode - Optional code to prepend before type checking
   * @throws {MontyTypingError} If type checking finds errors
   */
  typeCheck(prefixCode?: string): void

  /**
   * Executes the code and returns the result.
   * @param options - Execution options (inputs, limits)
   * @returns The result of the last expression
   * @throws {MontyRuntimeError} If the code raises an exception
   */
  run(options?: RunOptions): JsMontyObject

  /**
   * Starts execution and returns either a snapshot (paused at external call) or completion.
   *
   * This method enables iterative execution where code pauses at external function
   * calls, allowing the host to provide return values or exceptions before resuming.
   *
   * @param options - Execution options (inputs, limits)
   * @returns MontySnapshot if an external function call is pending, MontyComplete if done
   * @throws {MontyRuntimeError} If the code raises an exception
   */
  start(options?: StartOptions): MontySnapshot | MontyComplete

  /**
   * Serializes the Monty instance to a binary format.
   */
  dump(): Buffer

  /**
   * Deserializes a Monty instance from binary format.
   */
  static load(data: Buffer): Monty

  /** Returns the script name. */
  get scriptName(): string

  /** Returns the input variable names. */
  get inputs(): string[]

  /** Returns the external function names. */
  get externalFunctions(): string[]

  /** Returns a string representation of the Monty instance. */
  repr(): string
}

/**
 * Represents paused execution waiting for an external function call return value.
 *
 * Contains information about the pending external function call and allows
 * resuming execution with the return value or an exception.
 */
export declare class MontySnapshot {
  /** Returns the name of the script being executed. */
  get scriptName(): string

  /** Returns the name of the external function being called. */
  get functionName(): string

  /** Returns the positional arguments passed to the external function. */
  get args(): JsMontyObject[]

  /** Returns the keyword arguments passed to the external function as an object. */
  get kwargs(): Record<string, JsMontyObject>

  /**
   * Resumes execution with either a return value or an exception.
   *
   * Exactly one of `returnValue` or `exception` must be provided.
   *
   * @param options - Object with either `returnValue` or `exception`
   * @returns MontySnapshot if another external call is pending, MontyComplete if done
   * @throws {MontyRuntimeError} If the code raises an exception
   */
  resume(options: ResumeOptions): MontySnapshot | MontyComplete

  /**
   * Serializes the MontySnapshot to a binary format.
   *
   * The serialized data can be stored and later restored with `MontySnapshot.load()`.
   * This allows suspending execution and resuming later, potentially in a different process.
   */
  dump(): Buffer

  /**
   * Deserializes a MontySnapshot from binary format.
   *
   * @param data - The serialized snapshot data from `dump()`
   * @param options - Optional load options (reserved for future use)
   */
  static load(data: Buffer, options?: SnapshotLoadOptions): MontySnapshot

  /** Returns a string representation of the MontySnapshot. */
  repr(): string
}

/**
 * Represents completed execution with a final output value.
 */
export declare class MontyComplete {
  /** Returns the final output value from the executed code. */
  get output(): JsMontyObject

  /** Returns a string representation of the MontyComplete. */
  repr(): string
}
