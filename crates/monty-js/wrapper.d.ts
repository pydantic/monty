/**
 * Monty - A sandboxed Python interpreter for JavaScript/TypeScript.
 *
 * This module provides proper Error subclasses for exception handling.
 */

import type { MontyOptions, RunOptions, ResourceLimits, Frame, ExceptionInfo, RuntimeErrorInfo } from './index'

export type { MontyOptions, RunOptions, ResourceLimits, Frame, ExceptionInfo, RuntimeErrorInfo }

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
  run(options?: RunOptions): unknown

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
