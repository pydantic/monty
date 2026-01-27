// Custom error classes that extend Error for proper JavaScript error handling.
// These wrap the native Rust classes to provide instanceof support.

const native = require('./index.js')

// Re-export native classes for instanceof checks
const {
  Monty: NativeMonty,
  MontySnapshot: NativeMontySnapshot,
  MontyComplete: NativeMontyComplete,
  MontyException: NativeMontyException,
  MontyTypingError: NativeMontyTypingError,
} = native

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
   * @param {string | NativeMontyException} messageOrNative - The syntax error message or native exception
   */
  constructor(messageOrNative) {
    if (typeof messageOrNative === 'string') {
      super('SyntaxError', messageOrNative)
      this._native = null
    } else {
      // Native exception object
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
   * @param {'type-msg' | 'msg'} [format='msg'] - Output format
   * @returns {string}
   */
  display(format = 'msg') {
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
 *
 * @extends MontyError
 */
class MontyRuntimeError extends MontyError {
  /**
   * @param {NativeMontyException | string} nativeOrTypeName - Native exception or type name
   * @param {string} [message] - The error message (only if first arg is type name)
   * @param {string} [tracebackString] - The full traceback string (only if first arg is type name)
   * @param {Array<import('./index').Frame>} [frames] - The traceback frames (only if first arg is type name)
   */
  constructor(nativeOrTypeName, message, tracebackString, frames) {
    if (typeof nativeOrTypeName === 'string') {
      // Legacy constructor: (typeName, message, tracebackString, frames)
      super(nativeOrTypeName, message)
      this._native = null
      this._tracebackString = tracebackString
      this._frames = frames
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
   * @returns {Array<import('./index').Frame>}
   */
  traceback() {
    if (this._native) {
      return this._native.traceback()
    }
    return this._frames || []
  }

  /**
   * Returns formatted exception string.
   * @param {'traceback' | 'type-msg' | 'msg'} [format='traceback'] - Output format
   * @returns {string}
   */
  display(format = 'traceback') {
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
   * @param {string | NativeMontyTypingError} messageOrNative - The type error message or native error
   * @param {object} [nativeError] - Deprecated: The native MontyTypingError instance
   */
  constructor(messageOrNative, nativeError = null) {
    if (typeof messageOrNative === 'string') {
      super('TypeError', messageOrNative)
      this._native = nativeError
    } else {
      // Native error object
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
   * Renders the type error diagnostics with the specified format and color.
   *
   * @param {'full' | 'concise' | 'azure' | 'json' | 'jsonlines' | 'rdjson' | 'pylint' | 'gitlab' | 'github'} [format='full']
   * @param {boolean} [color=false] - Whether to include ANSI color codes
   * @returns {string}
   */
  display(format = 'full', color = false) {
    if (this._native && typeof this._native.display === 'function') {
      return this._native.display(format, color)
    }
    // Fallback if no native error
    return this._message
  }
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
   * @param {string} [prefixCode] - Optional code to prepend before type checking
   * @throws {MontyTypingError} If type checking finds errors
   */
  typeCheck(prefixCode) {
    const result = this._native.typeCheck(prefixCode)
    if (result instanceof NativeMontyTypingError) {
      throw new MontyTypingError(result)
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
    const result = this._native.run(options)
    if (result instanceof NativeMontyException) {
      throw new MontyRuntimeError(result)
    }
    return result
  }

  /**
   * Starts execution and returns either a snapshot (paused at external call) or completion.
   *
   * @param {import('./index').StartOptions} [options] - Execution options
   * @returns {MontySnapshot | MontyComplete}
   * @throws {MontyRuntimeError} If the code raises an exception
   */
  start(options) {
    const result = this._native.start(options)
    return wrapStartResult(result)
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

/**
 * Helper to wrap native start/resume results, throwing errors as needed.
 * @param {NativeMontySnapshot | NativeMontyComplete | NativeMontyException} result
 * @returns {MontySnapshot | MontyComplete}
 * @throws {MontyRuntimeError}
 */
function wrapStartResult(result) {
  if (result instanceof NativeMontyException) {
    throw new MontyRuntimeError(result)
  }
  if (result instanceof NativeMontySnapshot) {
    return new MontySnapshot(result)
  }
  if (result instanceof NativeMontyComplete) {
    return new MontyComplete(result)
  }
  // Fallback - shouldn't happen, but handle gracefully
  return result
}

/**
 * Represents paused execution waiting for an external function call return value.
 *
 * Contains information about the pending external function call and allows
 * resuming execution with the return value or an exception.
 */
class MontySnapshot {
  /**
   * @param {NativeMontySnapshot} nativeSnapshot - The native MontySnapshot instance
   */
  constructor(nativeSnapshot) {
    this._native = nativeSnapshot
  }

  /** @returns {string} */
  get scriptName() {
    return this._native.scriptName
  }

  /** @returns {string} */
  get functionName() {
    return this._native.functionName
  }

  /** @returns {any[]} */
  get args() {
    return this._native.args
  }

  /** @returns {Record<string, any>} */
  get kwargs() {
    return this._native.kwargs
  }

  /**
   * Resumes execution with either a return value or an exception.
   *
   * @param {import('./index').ResumeOptions} options - Object with either `returnValue` or `exception`
   * @returns {MontySnapshot | MontyComplete}
   * @throws {MontyRuntimeError} If the code raises an exception
   */
  resume(options) {
    const result = this._native.resume(options)
    return wrapStartResult(result)
  }

  /**
   * Serializes the MontySnapshot to a binary format.
   * @returns {Buffer}
   */
  dump() {
    return this._native.dump()
  }

  /**
   * Deserializes a MontySnapshot from binary format.
   * @param {Buffer} data
   * @param {import('./index').SnapshotLoadOptions} [options]
   * @returns {MontySnapshot}
   */
  static load(data, options) {
    const nativeSnapshot = NativeMontySnapshot.load(data, options)
    return new MontySnapshot(nativeSnapshot)
  }

  /** @returns {string} */
  repr() {
    return this._native.repr()
  }
}

/**
 * Represents completed execution with a final output value.
 */
class MontyComplete {
  /**
   * @param {NativeMontyComplete} nativeComplete - The native MontyComplete instance
   */
  constructor(nativeComplete) {
    this._native = nativeComplete
  }

  /** @returns {any} */
  get output() {
    return this._native.output
  }

  /** @returns {string} */
  repr() {
    return this._native.repr()
  }
}

/**
 * Runs a Monty script with async external function support.
 *
 * This function handles both synchronous and asynchronous external functions.
 * When an external function returns a Promise, it will be awaited before
 * resuming execution.
 *
 * @param {Monty} montyRunner - The Monty runner instance to execute
 * @param {Object} [options] - Execution options
 * @param {Record<string, any>} [options.inputs] - Input values for the script
 * @param {Record<string, Function>} [options.externalFunctions] - External function implementations (sync or async)
 * @param {import('./index').JsResourceLimits} [options.limits] - Resource limits
 * @returns {Promise<any>} The output of the Monty script
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
async function runMontyAsync(montyRunner, options = {}) {
  const { inputs, externalFunctions = {}, limits } = options

  let progress = montyRunner.start({ inputs, limits })

  while (true) {
    if (progress instanceof MontyComplete) {
      return progress.output
    }

    if (progress instanceof MontySnapshot) {
      const funcName = progress.functionName
      const extFunction = externalFunctions[funcName]

      if (!extFunction) {
        // Function not found - resume with a KeyError exception
        progress = progress.resume({
          exception: {
            type: 'KeyError',
            message: `"External function '${funcName}' not found"`,
          },
        })
        continue
      }

      try {
        // Call the external function
        let result = extFunction(...progress.args, progress.kwargs)

        // If the result is a Promise, await it
        if (result && typeof result.then === 'function') {
          result = await result
        }

        // Resume with the return value
        progress = progress.resume({ returnValue: result })
      } catch (error) {
        // External function threw an exception - convert to Monty exception
        const excType = error.name || 'RuntimeError'
        const excMessage = error.message || String(error)
        progress = progress.resume({
          exception: {
            type: excType,
            message: excMessage,
          },
        })
      }
    } else {
      // Unexpected progress type
      throw new Error(`Unexpected progress type: ${progress}`)
    }
  }
}

module.exports = {
  // Main class
  Monty,
  // Iterative execution classes
  MontySnapshot,
  MontyComplete,
  // Error classes
  MontyError,
  MontySyntaxError,
  MontyRuntimeError,
  MontyTypingError,
  // Async execution helper
  runMontyAsync,
  // Re-export types/interfaces (these are just for documentation, not actual values)
}

// Also export as ES module compatible
module.exports.default = module.exports
