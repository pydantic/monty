// index-header.d.ts - header will be written into index.d.ts on build

type JsMontyObject = any
/**
 * A sandboxed Python interpreter instance.
 *
 * Parses and compiles Python code on initialization, then can be run
 * multiple times with different input values. This separates the parsing
 * cost from execution, making repeated runs more efficient.
 */
export declare class Monty {
  /**
   * Creates a new Monty interpreter by parsing the given code.
   *
   * Returns either a Monty instance, a MontyException (for syntax errors), or a MontyTypingError.
   * The wrapper should check the result type and throw the appropriate error.
   *
   * @param code - Python code to execute
   * @param options - Configuration options
   * @returns Monty instance on success, or error object on failure
   */
  static create(code: string, options?: MontyOptions | undefined | null): Self | MontyException | MontyTypingError
  /**
   * Performs static type checking on the code.
   *
   * Returns either nothing (success) or a MontyTypingError.
   *
   * @param prefixCode - Optional code to prepend before type checking
   * @returns null on success, or MontyTypingError on failure
   */
  typeCheck(prefixCode?: string | undefined | null): MontyTypingError | null
  /**
   * Executes the code, returning the last expression's result or a
   * MontyException on failure. With a runtime `externalLookup` or mounts,
   * dispatches calls/lookups via the start/resume loop; otherwise runs
   * directly.
   *
   * @param options - Execution options (inputs, limits, externalLookup)
   * @returns The result of the last expression, or a MontyException if execution fails
   */
  run(options?: RunOptions | undefined | null): JsMontyObject | MontyException
  /**
   * Starts iterative execution, pausing at external function calls or name
   * lookups so the host can supply a value before resuming. Returns a
   * snapshot, completion, or error.
   *
   * @param options - Execution options (inputs, limits)
   * @returns MontySnapshot if paused at function call, MontyNameLookup if paused at
   *   name lookup, MontyComplete if done, or MontyException if failed
   */
  start(options?: StartOptions | undefined | null): MontySnapshot | MontyNameLookup | MontyComplete | MontyException
  /**
   * Serializes the Monty instance to bytes (restore with `Monty.load()`),
   * caching parsed code to avoid re-parsing on later runs.
   *
   * @returns Buffer containing the serialized Monty instance
   */
  dump(): Buffer
  /**
   * Deserializes a Monty instance from binary format.
   *
   * @param data - The serialized Monty data from `dump()`
   * @returns A new Monty instance
   */
  static load(data: Buffer): Monty
  /** Returns the script name. */
  get scriptName(): string
  /** Returns the input variable names. */
  get inputs(): Array<string>
  /** Returns a string representation of the Monty instance. */
  repr(): string
}

/**
 * Represents completed execution with a final output value.
 *
 * The output value is stored as a `MontyObject` internally and converted to JS on access.
 */
export declare class MontyComplete {
  /** Returns the final output value from the executed code. */
  get output(): JsMontyObject
  /** Returns a string representation of the MontyComplete. */
  repr(): string
}

/**
 * Wrapper around core `MontyException` for napi bindings.
 *
 * This is a thin newtype wrapper that exposes the necessary getters for the
 * JavaScript wrapper to construct appropriate error types (`MontySyntaxError`
 * or `MontyRuntimeError`) based on the exception type.
 */
export declare class MontyException {
  /**
   * Returns information about the inner Python exception.
   *
   * The `typeName` field can be used to distinguish syntax errors (`"SyntaxError"`)
   * from runtime errors (e.g., `"ValueError"`, `"TypeError"`).
   */
  get exception(): ExceptionInfo
  /** Returns the error message. */
  get message(): string
  /**
   * Returns the Monty traceback as an array of Frame objects.
   *
   * For syntax errors, this will be an empty array.
   * For runtime errors, this contains the stack frames where the error occurred.
   *
   * `Frame.source_line` is built as a `JsString` shared across frames that
   * resolve to the same source line. For deep recursion where every frame
   * points at the same line this creates a single V8 string referenced by
   * every frame, instead of one copy per frame.
   */
  traceback(): Array<Frame>
  /**
   * Returns formatted exception string.
   *
   * @param format - Output format:
   *   - 'traceback' - Full traceback (default)
   *   - 'type-msg' - 'ExceptionType: message' format
   *   - 'msg' - just the message
   */
  display(format?: string | undefined | null): string
  /** Returns a string representation of the error. */
  toString(): string
}
export type JsMontyException = MontyException

/**
 * Represents paused execution waiting for a name to be resolved.
 *
 * The host should check if the variable name corresponds to a known value
 * (e.g., an external function). Call `resume()` with the value to continue
 * execution, or call `resume()` with no value to raise `NameError`.
 */
export declare class MontyNameLookup {
  /** Returns the name of the script being executed. */
  get scriptName(): string
  /** Returns the name of the variable being looked up. */
  get variableName(): string
  /**
   * Resumes execution after resolving the name lookup.
   *
   * If `value` is provided, the name resolves to that value and execution continues.
   * If `value` is omitted or undefined, the VM raises a `NameError`.
   *
   * @param options - Optional object with `value` to resolve the name to
   * @returns MontySnapshot if paused at function call, MontyNameLookup if paused at
   *   another name lookup, MontyComplete if done, or MontyException if failed
   */
  resume(options?: NameLookupResumeOptions | undefined | null): MontySnapshot | Self | MontyComplete | MontyException
  /**
   * Serializes the MontyNameLookup to a binary format.
   *
   * The serialized data can be stored and later restored with `MontyNameLookup.load()`.
   *
   * @returns Buffer containing the serialized name lookup snapshot
   */
  dump(): Buffer
  /**
   * Deserializes a MontyNameLookup from binary format.
   *
   * @param data - The serialized data from `dump()`
   * @param options - Optional load options
   * @returns A new MontyNameLookup instance
   */
  static load(data: Buffer, options?: NameLookupLoadOptions | undefined | null): MontyNameLookup
  /** Returns a string representation of the MontyNameLookup. */
  repr(): string
}

/**
 * Stateful no-replay REPL session.
 *
 * Create with `new MontyRepl()` then call `feed()` to execute snippets
 * incrementally against persistent heap and namespace state.
 */
export declare class MontyRepl {
  /**
   * Creates an empty REPL session ready to receive snippets via `feed()`.
   *
   * No code is parsed or executed at construction time — all execution
   * is driven through `feed()`.
   *
   * @param options - Optional configuration (scriptName, limits)
   */
  constructor(options?: MontyReplOptions | undefined | null)
  /** Returns the script name for this REPL session. */
  get scriptName(): string
  /**
   * Executes one incremental snippet against persistent REPL state.
   *
   * @param code - Python code to execute
   * @param options - Optional feed options (mount)
   */
  feed(code: string, options?: FeedOptions | undefined | null): JsMontyObject | MontyException
  /** Serializes this REPL session to bytes. */
  dump(): Buffer
  /** Restores a REPL session from bytes produced by `dump()`. */
  static load(data: Buffer): MontyRepl
  /** Returns a string representation of the REPL session. */
  repr(): string
}

/**
 * Paused execution waiting for an external function call return value, with
 * the pending call's details and the state needed to resume it.
 */
export declare class MontySnapshot {
  /** Returns the name of the script being executed. */
  get scriptName(): string
  /** Returns the name of the external function being called. */
  get functionName(): string
  /** Returns the positional arguments passed to the external function. */
  get args(): Array<JsMontyObject>
  /** Returns the keyword arguments passed to the external function as an object. */
  get kwargs(): object
  /**
   * Resumes execution with either a return value or an exception.
   *
   * Exactly one of `returnValue` or `exception` must be provided.
   *
   * @param options - Object with either `returnValue` or `exception`
   * @returns MontySnapshot if paused at function call, MontyNameLookup if paused at
   *   name lookup, MontyComplete if done, or MontyException if failed
   */
  resume(options: ResumeOptions): Self | MontyNameLookup | MontyComplete | MontyException
  /**
   * Serializes the snapshot to bytes (restore with `MontySnapshot.load()`),
   * so execution can be suspended and resumed later, even in another process.
   *
   * @returns Buffer containing the serialized snapshot
   */
  dump(): Buffer
  /**
   * Deserializes a MontySnapshot from binary format.
   *
   * @param data - The serialized snapshot data from `dump()`
   * @param options - Optional load options (reserved for future use)
   * @returns A new MontySnapshot instance
   */
  static load(data: Buffer, options?: SnapshotLoadOptions | undefined | null): MontySnapshot
  /** Returns a string representation of the MontySnapshot. */
  repr(): string
}

/**
 * Raised when type checking finds errors in the code.
 *
 * This exception is raised when static type analysis detects type errors.
 * Use `display()` to render diagnostics in various formats.
 */
export declare class MontyTypingError {
  /** Returns information about the inner exception. */
  get exception(): ExceptionInfo
  /** Returns the error message. */
  get message(): string
  /**
   * Renders the type error diagnostics with the specified format and color.
   *
   * @param format - Output format. One of:
   *   - 'full' - Full diagnostic output (default)
   *   - 'concise' - Concise output
   *   - 'azure' - Azure DevOps format
   *   - 'json' - JSON format
   *   - 'jsonlines' - JSON Lines format
   *   - 'rdjson' - RDJson format
   *   - 'pylint' - Pylint format
   *   - 'gitlab' - GitLab CI format
   *   - 'github' - GitHub Actions format
   * @param color - Whether to include ANSI color codes. Default: false
   */
  display(format?: string | undefined | null, color?: boolean | undefined | null): string
  /** Returns a string representation of the error. */
  toString(): string
}

/**
 * A single mount point mapping a virtual path to a host directory.
 *
 * Owns the underlying [`Mount`] via shared storage. In the native subprocess
 * API this is reusable configuration; `'overlay'` writes live only for the
 * current feed. In the wasm in-process API, the mount is temporarily taken
 * while `Monty.run()` / `Monty.start()` executes.
 *
 * The `mode` controls sandbox access:
 * - `'read-only'` — sandbox can read but not write
 * - `'read-write'` — sandbox can read and write real host files
 * - `'overlay'` — reads fall through to host; writes are captured in memory
 */
export declare class MountDir {
  /**
   * Creates a new mount directory.
   *
   * @param virtualPath - Absolute virtual path prefix (e.g. `"/data"`)
   * @param hostPath - Path to the real host directory
   * @param options - Optional mode and write_bytes_limit
   */
  constructor(virtualPath: string, hostPath: string, options?: MountDirOptions | undefined | null)
  /** The normalized virtual path prefix inside the sandbox. */
  get virtualPath(): string
  /** The canonical host directory path. */
  get hostPath(): string
  /** The access mode: `"read-only"`, `"read-write"`, or `"overlay"`. */
  get mode(): string
  /** The optional write bytes limit, or `null` if unlimited. */
  get writeBytesLimit(): number | null
  /**
   * Returns a string representation of this mount directory.
   *
   * # Panics
   *
   * Panics if the internal mutex is poisoned.
   */
  repr(): string
}

/**
 * A pool of `monty` worker subprocesses. Wrapped by the TypeScript `Monty`
 * class — not part of the public API.
 */
export declare class NativePool {
  /**
   * Validates and stores the configuration; workers are spawned by
   * [`start`](Self::start).
   */
  constructor(options: NativePoolOptions)
  /** Spawns the prewarmed workers off the event loop. */
  start(): Promise<undefined>
  /** Prepares a session; its worker is checked out by `NativeSession.enter`. */
  checkout(options: NativeCheckoutOptions): NativeSession
  /**
   * Shuts the pool down: idle workers exit, capacity is gone. Sessions
   * still checked out keep their workers until they finish.
   */
  close(): Promise<undefined>
}

/**
 * One worker process dedicated to one REPL session. Wrapped by the
 * TypeScript `MontySession` class — not part of the public API.
 */
export declare class NativeSession {
  /**
   * Checks a worker out of the pool (spawning one if allowed) and creates
   * the REPL session in it. Rejects with the pool error message on
   * exhaustion or spawn failure.
   */
  enter(): Promise<undefined>
  /**
   * Runs one feed turn: executes `code` until completion or the first
   * suspension, streaming prints to `on_print`. Resolves to a turn object.
   */
  feed(code: string, inputs: object | undefined | null, mounts: Array<NativeMount>, skipTypeCheck: boolean, onPrint: PrintCallback): Promise<object>
  /**
   * Answers a `functionCall`/`osCall` suspension with a return value. A
   * value that cannot cross the wire becomes a catchable in-sandbox error
   * instead — this method never fails for value reasons, because the
   * worker is suspended awaiting exactly one resume.
   */
  resumeReturn(value: unknown, onPrint: PrintCallback): Promise<object>
  /**
   * Answers a suspension with an exception (`excType` must be a Python
   * exception type name monty knows; anything else becomes RuntimeError).
   */
  resumeError(excType: string, message: string, onPrint: PrintCallback): Promise<object>
  /**
   * Answers an `osCall` suspension by declining it: the sandbox raises the
   * call's default exception (full traceback preserved Rust-side).
   */
  resumeNotHandled(onPrint: PrintCallback): Promise<object>
  /**
   * Answers a `functionCall` suspension whose name has no handler: the
   * sandbox raises `NameError`.
   */
  resumeNotFound(onPrint: PrintCallback): Promise<object>
  /**
   * Registers the pending call as an external future (the JS promise stays
   * in TypeScript); other sandbox tasks keep executing.
   */
  resumeFuture(onPrint: PrintCallback): Promise<object>
  /**
   * Answers a `nameLookup` suspension against `externalLookup`. A callable
   * entry resolves to a host function proxy, passed here as its display name
   * (`function_name`); any other entry is passed inside the `value` wrapper
   * (`{ value: ... }`) and converted to a wire value returned directly. The
   * wrapper exists because napi maps a bare JS `null`/`undefined` argument to
   * "absent" — without it, an entry whose value *is* `null` would be
   * indistinguishable from an undefined name. With both arguments absent the
   * name is undefined and the sandbox raises `NameError`. A `value` that
   * cannot cross the wire rejects the turn (the worker has not yet observed
   * the name).
   */
  resumeNameLookup(functionName: string | undefined | null, value: object | undefined | null, onPrint: PrintCallback): Promise<object>
  /**
   * Answers a `resolveFutures` suspension with the settled promises'
   * outcomes: an array of `{ callId, ok, value?, excType?, message? }`.
   */
  resolveFutures(results: Array<object>, onPrint: PrintCallback): Promise<object>
  /**
   * Restores a dump into this session's freshly configured worker. Resolves
   * to a turn object: a suspension when the dump was mid-feed, or `loaded`
   * for an idle dump. The TypeScript `load` / `loadSnapshot` split inspects
   * the kind and enforces "fresh session only".
   */
  restore(state: Buffer, mounts: Array<NativeMount>, onPrint: PrintCallback): Promise<object>
  /**
   * Serializes the worker's session state (idle or suspended) into opaque
   * bytes via monty's dump format. The session stays usable.
   */
  dump(): Promise<Buffer>
  /**
   * Installs third-party Python packages into the session via the worker's
   * `uv`, making them importable by later feeds. Session-scoped and
   * repeatable. Resolves to a turn object: `{kind:'ok'}` on success, or an
   * `error` / `crashed` / `protocol` outcome the TypeScript layer raises
   * (a uv failure, or the `monty` sandbox worker rejecting the request,
   * arrives as `error`). Streams no prints, but takes `on_print` to share the
   * turn machinery; the callback is never invoked.
   */
  installDependencies(requirements: Array<string>, onPrint: PrintCallback): Promise<object>
  /**
   * Ends the session and returns the worker to the pool (best effort — a
   * crashed worker has already been discarded and replaced).
   */
  finish(): Promise<undefined>
  /**
   * OS process id of this session's worker, or `null` when no worker is
   * attached or a turn is in flight (the turn thread holds the checkout
   * lock — blocking the event loop on it would deadlock with the print
   * callback, which needs the event loop).
   */
  get workerPid(): number | null
}

/**
 * Information about the inner Python exception.
 *
 * This provides structured access to the exception type and message
 * for programmatic error handling.
 */
export interface ExceptionInfo {
  /** The exception type name (e.g., "ValueError", "TypeError", "SyntaxError"). */
  typeName: string
  /** The exception message. */
  message: string
}

/** Input for raising an exception during resume. */
export interface ExceptionInput {
  /** The exception type name (e.g., "ValueError"). */
  type: string
  /** The exception message. */
  message: string
}

/** Options for `MontyRepl.feed()`. */
export interface FeedOptions {
  /**
   * Filesystem mount(s) for the sandbox.
   * A single `MountDir` or an array of `MountDir`.
   */
  mount?: object
}

/**
 * A single frame in a Monty traceback.
 *
 * Contains all the information needed to display a traceback line:
 * the file location, function name, and optional source code preview.
 *
 * `source_line` is a `JsString` borrowed from the env scope of the
 * `traceback()` call that produced this frame. Frames produced by the same
 * `traceback()` call that resolve to the same source location share one V8
 * string allocation. The lifetime parameter ties the frame to that env
 * scope, since `JsString<'env>` is a non-owning handle.
 */
export interface Frame {
  /** The filename where the code is located. */
  filename: string
  /** Line number (1-based). */
  line: number
  /** Column number (1-based). */
  column: number
  /** End line number (1-based). */
  endLine: number
  /** End column number (1-based). */
  endColumn: number
  /** The name of the function, or null for module-level code. */
  functionName?: string
  /** The source code line for preview in the traceback. */
  sourceLine?: string
}

/**
 * Deepest *list-like* value nesting the wire protocol accepts (dicts and
 * dataclasses cost more recursion budget per level, so nest less deeply).
 */
export const MAX_VALUE_DEPTH: number

/** Options for creating a new Monty instance. */
export interface MontyOptions {
  /** Name used in tracebacks and error messages. Default: 'main.py' */
  scriptName?: string
  /** List of input variable names available in the code. */
  inputs?: Array<string>
  /** Whether failed asserts include introspected messages. Default: true */
  assertMessageAnnotations?: boolean
  /** Whether to perform type checking on the code. Default: false */
  typeCheck?: boolean
  /** Optional code to prepend before type checking. */
  typeCheckPrefixCode?: string
}

/**
 * Options for creating a new `MontyRepl` instance.
 *
 * Controls the script name shown in tracebacks and optional resource limits
 * that apply to all subsequent `feed()` calls.
 */
export interface MontyReplOptions {
  /** Name used in tracebacks and error messages. Default: 'main.py' */
  scriptName?: string
  /** Resource limits configuration applied to all snippet executions. */
  limits?: ResourceLimits
  /** Whether failed asserts include introspected messages. Default: true */
  assertMessageAnnotations?: boolean
}

/** Options for creating a new MountDir. */
export interface MountDirOptions {
  /** Access mode: `'read-only'`, `'read-write'`, or `'overlay'` (default). */
  mode?: string
  /** Optional limit on cumulative bytes written through this mount. */
  writeBytesLimit?: number
}

/** Options for loading a serialized name lookup snapshot. */
export interface NameLookupLoadOptions {
  /** Optional print callback function. */
  printCallback?: JsPrintCallback
}

/**
 * Options for resuming execution from a name lookup.
 *
 * If `value` is provided, the name resolves to that value and execution continues.
 * If `value` is omitted or undefined, the VM raises a `NameError`.
 */
export interface NameLookupResumeOptions {
  /** The value to provide for the name. */
  value?: unknown
}

/** Session options for `checkout()`. */
export interface NativeCheckoutOptions {
  /** Script name used in tracebacks and type-check diagnostics. */
  scriptName: string
  /** Sandbox resource limits enforced inside the worker. */
  limits?: ResourceLimits
  /** Type-check each fed snippet before executing it. */
  typeCheck: boolean
  /** Stub declarations made available to type checking. */
  typeCheckStubs?: string
  /**
   * Give failed `assert` statements pytest-style introspected messages
   * (see limitations/assert.md). On by default.
   */
  assertMessageAnnotations: boolean
}

/** One mount entry for a feed, pre-validated by the TypeScript `MountDir`. */
export interface NativeMount {
  /** Absolute virtual POSIX path inside the sandbox, e.g. `/mnt/data`. */
  virtualPath: string
  /** Host directory to expose. */
  hostPath: string
  /** `'read-only'`, `'read-write'` or `'overlay'`. */
  mode: string
  /** Cap on total bytes written through this mount. */
  writeBytesLimit?: number
}

/**
 * Pool construction options. Timeouts are pre-normalised to milliseconds by
 * the TypeScript layer (which also applies the `durationLimitGrace` default
 * and resolves the binary path).
 */
export interface NativePoolOptions {
  /** Resolved path to the `monty` binary. */
  binaryPath: string
  /** Workers spawned eagerly by `start()` and kept warm. */
  minProcesses: number
  /** Hard cap on live workers; checkouts beyond it wait. */
  maxProcesses: number
  /** How long `enter()` waits for a free worker (ms). Absent: forever. */
  checkoutTimeoutMs?: number
  /** Parent-side hard deadline per protocol turn (ms). */
  requestTimeoutMs?: number
  /**
   * Grace for the automatic `maxDurationSecs` backstop (ms). Absent:
   * backstop disabled.
   */
  durationLimitGraceMs?: number
  /** Recycle a worker after serving this many checkouts. */
  maxCheckoutsPerWorker?: number
}

/**
 * Resource limits configuration from JavaScript.
 *
 * All limits are optional. Omit a key to disable that limit.
 * Numeric limits are received as JS `number`s, so the boundary uses `f64`
 * and validates them before converting into Rust `usize` values.
 */
export interface ResourceLimits {
  /** Maximum number of heap allocations allowed. */
  maxAllocations?: number
  /** Maximum execution time in seconds. */
  maxDurationSecs?: number
  /** Maximum heap memory in bytes. */
  maxMemory?: number
  /** Run garbage collection every N allocations. */
  gcInterval?: number
  /** Maximum function call stack depth (default: 1000). */
  maxRecursionDepth?: number
}

/** Options for resuming execution. */
export interface ResumeOptions {
  /** The value to return from the external function call. */
  returnValue?: unknown
  /**
   * An exception to raise in the interpreter.
   * Format: { type: string, message: string }
   */
  exception?: ExceptionInput
}

/** Options for running code. */
export interface RunOptions {
  inputs?: object
  /** Resource limits configuration. */
  limits?: ResourceLimits
  /** Optional print callback function. */
  printCallback?: JsPrintCallback
  /**
   * Lazy resolution for names the code leaves undefined. Keys are names; a
   * callable value resolves to a host function proxy, any other value is
   * converted and returned directly, and an absent name raises `NameError`.
   */
  externalLookup?: object
  /**
   * Filesystem mount(s) for the sandbox.
   * A single `MountDir` or an array of `MountDir`.
   */
  mount?: object
}

/** Options for loading a serialized snapshot. */
export interface SnapshotLoadOptions {
  /** Optional print callback function. */
  printCallback?: JsPrintCallback
}

/** Options for starting execution. */
export interface StartOptions {
  /** Dict of input variable values. */
  inputs?: object
  /** Resource limits configuration. */
  limits?: ResourceLimits
  /** Optional print callback function. */
  printCallback?: JsPrintCallback
  /**
   * Filesystem mount(s) for the sandbox.
   * A single `MountDir` or an array of `MountDir`.
   */
  mount?: object
}
