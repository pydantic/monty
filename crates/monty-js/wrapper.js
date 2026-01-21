// Custom error classes that extend Error for proper JavaScript error handling.
// These wrap the native Rust classes to provide instanceof support.

const native = require('./index.js')

/**
 * Base class for all Monty interpreter errors.
 *
 * This is the parent class for `MontySyntaxError`, `MontyRuntimeError`, and `MontyTypingError`.
 * Catching `MontyError` will catch any exception raised by Monty.
 *
 * @extends Error
 */
class MontyError extends Error {
  /**
   * @param {string} typeName - The Python exception type name
   * @param {string} message - The error message
   */
  constructor(typeName, message) {
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
   * @returns {{ typeName: string, message: string }}
   */
  get exception() {
    return {
      typeName: this._typeName,
      message: this._message,
    }
  }

  /**
   * Returns formatted exception string.
   * @param {'type-msg' | 'msg'} [format='msg'] - Output format
   * @returns {string}
   */
  display(format = 'msg') {
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
 *
 * @extends MontyError
 */
class MontySyntaxError extends MontyError {
  /**
   * @param {string} message - The syntax error message
   */
  constructor(message) {
    super('SyntaxError', message)
    this.name = 'MontySyntaxError'
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, MontySyntaxError)
    }
  }
}

/**
 * Raised when Monty code fails during execution.
 *
 * Provides access to the traceback frames where the error occurred via `traceback()`,
 * and formatted output via `display()`.
 *
 * @extends MontyError
 */
class MontyRuntimeError extends MontyError {
  /**
   * @param {string} typeName - The Python exception type name
   * @param {string} message - The error message
   * @param {string} tracebackString - The full traceback string
   * @param {Array<import('./index').Frame>} frames - The traceback frames
   */
  constructor(typeName, message, tracebackString, frames) {
    super(typeName, message)
    this.name = 'MontyRuntimeError'
    this._tracebackString = tracebackString
    this._frames = frames
    // Override the message to include the full traceback
    this.message = tracebackString
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, MontyRuntimeError)
    }
  }

  /**
   * Returns the Monty traceback as an array of Frame objects.
   * @returns {Array<import('./index').Frame>}
   */
  traceback() {
    return this._frames
  }

  /**
   * Returns formatted exception string.
   * @param {'traceback' | 'type-msg' | 'msg'} [format='traceback'] - Output format
   * @returns {string}
   */
  display(format = 'traceback') {
    switch (format) {
      case 'traceback':
        return this._tracebackString
      case 'type-msg':
        return this._message ? `${this._typeName}: ${this._message}` : this._typeName
      case 'msg':
        return this._message
      default:
        throw new Error(`Invalid display format: '${format}'. Expected 'traceback', 'type-msg', or 'msg'`)
    }
  }
}

/**
 * Raised when type checking finds errors in the code.
 *
 * This exception is raised when static type analysis detects type errors.
 * Use `display()` to render diagnostics in various formats.
 *
 * @extends MontyError
 */
class MontyTypingError extends MontyError {
  /**
   * @param {string} message - The type error message
   * @param {object} nativeError - The native MontyTypingError instance (for display formatting)
   */
  constructor(message, nativeError = null) {
    super('TypeError', message)
    this.name = 'MontyTypingError'
    this._nativeError = nativeError
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, MontyTypingError)
    }
  }

  /**
   * Renders the type error diagnostics with the specified format and color.
   *
   * @param {'full' | 'concise' | 'azure' | 'json' | 'jsonlines' | 'rdjson' | 'pylint' | 'gitlab' | 'github'} [format='full']
   * @param {boolean} [color=false] - Whether to include ANSI color codes
   * @returns {string}
   */
  display(format = 'full', color = false) {
    if (this._nativeError && typeof this._nativeError.display === 'function') {
      return this._nativeError.display(format, color)
    }
    // Fallback if no native error
    return this._message
  }
}

// Re-export the native Monty class and other exports
const { Monty: NativeMonty } = native

/**
 * Helper to parse error messages and create appropriate error instances.
 * @param {Error} error - The error thrown by the native module
 * @returns {MontyError}
 */
function wrapNativeError(error) {
  const message = error.message || ''

  // Check for syntax errors
  if (message.startsWith('SyntaxError:')) {
    return new MontySyntaxError(message.replace('SyntaxError: ', ''))
  }

  // Check for type errors from type checking
  if (message.startsWith('TypeError:')) {
    return new MontyTypingError(message.replace('TypeError: ', ''))
  }

  // Check for runtime errors with traceback
  if (message.includes('Traceback (most recent call last):')) {
    // Parse the traceback to extract exception info
    const lines = message.split('\n')
    const lastLine = lines[lines.length - 1]
    const colonIndex = lastLine.indexOf(':')
    let typeName = lastLine
    let msg = ''
    if (colonIndex !== -1) {
      typeName = lastLine.substring(0, colonIndex)
      msg = lastLine.substring(colonIndex + 2) // Skip ': '
    }
    // We don't have frame info from a plain error, pass empty array
    return new MontyRuntimeError(typeName, msg, message, [])
  }

  // Check for other known exception types
  const exceptionPatterns = [
    'ValueError',
    'TypeError',
    'KeyError',
    'IndexError',
    'NameError',
    'AttributeError',
    'ZeroDivisionError',
    'RuntimeError',
    'RecursionError',
    'AssertionError',
    'OverflowError',
    'MemoryError',
    'NotImplementedError',
    'ImportError',
    'ModuleNotFoundError',
  ]

  for (const pattern of exceptionPatterns) {
    if (message.startsWith(`${pattern}:`)) {
      return new MontyRuntimeError(pattern, message.substring(pattern.length + 2), message, [])
    }
  }

  // Generic MontyError fallback
  return new MontyError('Error', message)
}

/**
 * Wrapped Monty class that throws proper Error subclasses.
 */
class Monty {
  /**
   * Creates a new Monty interpreter by parsing the given code.
   *
   * @param {string} code - Python code to execute
   * @param {import('./index').MontyOptions} [options] - Configuration options
   * @throws {MontySyntaxError} If the code has syntax errors
   * @throws {MontyTypingError} If type checking is enabled and finds errors
   */
  constructor(code, options) {
    try {
      this._native = new NativeMonty(code, options)
    } catch (error) {
      throw wrapNativeError(error)
    }
  }

  /**
   * Performs static type checking on the code.
   *
   * @param {string} [prefixCode] - Optional code to prepend before type checking
   * @throws {MontyTypingError} If type checking finds errors
   */
  typeCheck(prefixCode) {
    try {
      this._native.typeCheck(prefixCode)
    } catch (error) {
      throw wrapNativeError(error)
    }
  }

  /**
   * Executes the code and returns the result.
   *
   * @param {import('./index').RunOptions} [options] - Execution options
   * @returns {any} The result of the last expression
   * @throws {MontyRuntimeError} If the code raises an exception
   */
  run(options) {
    try {
      return this._native.run(options)
    } catch (error) {
      throw wrapNativeError(error)
    }
  }

  /**
   * Serializes the Monty instance to a binary format.
   * @returns {Buffer}
   */
  dump() {
    return this._native.dump()
  }

  /**
   * Deserializes a Monty instance from binary format.
   * @param {Buffer} data
   * @returns {Monty}
   */
  static load(data) {
    const instance = Object.create(Monty.prototype)
    instance._native = NativeMonty.load(data)
    return instance
  }

  /** @returns {string} */
  get scriptName() {
    return this._native.scriptName
  }

  /** @returns {string[]} */
  get inputs() {
    return this._native.inputs
  }

  /** @returns {string[]} */
  get externalFunctions() {
    return this._native.externalFunctions
  }

  /** @returns {string} */
  repr() {
    return this._native.repr()
  }
}

module.exports = {
  // Main class
  Monty,
  // Error classes
  MontyError,
  MontySyntaxError,
  MontyRuntimeError,
  MontyTypingError,
  // Re-export types/interfaces (these are just for documentation, not actual values)
}

// Also export as ES module compatible
module.exports.default = module.exports
