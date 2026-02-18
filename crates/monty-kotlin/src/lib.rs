//! Kotlin/JVM bindings for the Monty sandboxed Python interpreter via UniFFI.
//!
//! This crate exposes Monty to Kotlin (and other JVM languages) using Mozilla's
//! UniFFI binding generator. It follows the same pause/resume execution model as
//! the Python and JavaScript bindings, allowing Kotlin hosts to handle external
//! function callbacks — such as calling MCP servers or other external services.
//!
//! ## Design
//!
//! Values cross the FFI boundary as JSON strings to avoid the complexity of
//! recursive enums in UniFFI. Kotlin consumers work with standard JSON
//! (using any JSON library they prefer).
//!
//! The execution loop runs entirely inside Rust: Kotlin implements
//! `ExternalFunctionHandler` to respond to external function calls, and
//! `MontyKt::run()` drives the loop.
//!
//! ## Quick Start (Kotlin)
//!
//! ```kotlin
//! val m = MontyKt.create("x + 1", null, listOf("x"), null, null)
//! val result = m.run("""{"x": 41}""", noopHandler)
//! // result == "42"
//! ```
//!
//! ## Type Checking (Kotlin)
//!
//! Pass a prefix string to `create()` to enable static type checking before
//! execution. This is useful for LLM-generated code where you want to catch
//! type errors early.
//!
//! ```kotlin
//! try {
//!     MontyKt.create("x: int = 'oops'", null, null, null, Some(""))
//! } catch (e: MontyException.TypingException) {
//!     println("Type error: ${e.reason}")
//! }
//! ```

use std::sync::Arc;
use std::time::Duration;

use monty::{
    ExternalResult, LimitedTracker, MontyException, MontyObject, MontyRun, NoLimitTracker, PrintWriter,
    ResourceLimits, ResourceTracker, RunProgress,
};
use monty_type_checking::{SourceFile, type_check};
use serde_json::Value as JsonValue;

// Pull in the UniFFI scaffolding generated from monty_kotlin.udl.
uniffi::include_scaffolding!("monty_kotlin");

// =============================================================================
// Error type
// =============================================================================

/// Errors that can occur during Monty code parsing, type checking, or execution.
///
/// Each variant carries a `reason: String` field (not `message`, to avoid
/// conflicting with `Throwable.message` in Kotlin). UniFFI auto-generates
/// `override val message get() = "reason=${reason}"` so that `e.message` works
/// as expected, but `e.reason` gives direct access to the error text.
///
/// Using a named field (rather than `#[uniffi(flat_error)]`) makes this error
/// type fully bidirectional: Kotlin callbacks may throw typed
/// `MontyException.RuntimeException(reason = "…")` and Rust can lift it back.
///
/// In Kotlin, these surface as a sealed class hierarchy:
/// - `MontyException.SyntaxException(reason)`
/// - `MontyException.TypingException(reason)`
/// - `MontyException.RuntimeException(reason)`
/// - `MontyException.OsCallNotSupported(reason)`
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MontyError {
    /// The provided Python code could not be parsed.
    #[error("SyntaxError: {reason}")]
    SyntaxError {
        /// Human-readable description of the syntax problem.
        reason: String,
    },
    /// Static type checking found errors in the code.
    ///
    /// Only returned when `type_check_prefix` is provided to `MontyKt::create()`.
    #[error("TypingError:\n{reason}")]
    TypingError {
        /// The formatted type-checking diagnostics, ready for display to the user.
        reason: String,
    },
    /// A runtime error occurred during execution (e.g., `ZeroDivisionError`).
    #[error("RuntimeError: {reason}")]
    RuntimeError {
        /// Human-readable description of the runtime error.
        reason: String,
    },
    /// The sandboxed code attempted an OS-level operation (e.g., filesystem, network).
    ///
    /// Monty intentionally blocks OS calls in this binding; they should be routed
    /// through `ExternalFunctionHandler` instead.
    #[error("OsCallNotSupported: {reason}")]
    OsCallNotSupported {
        /// Description of the OS function that was attempted.
        reason: String,
    },
}

/// Maps an unexpected UniFFI callback error (e.g., the Kotlin side threw an
/// unhandled exception) to a `MontyError::RuntimeError`.
///
/// Required by UniFFI for any error type used with `callback_interface`.
impl From<uniffi::UnexpectedUniFFICallbackError> for MontyError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        Self::RuntimeError {
            reason: format!("Unexpected callback error: {}", e.reason),
        }
    }
}

// =============================================================================
// Resource limits
// =============================================================================

/// Optional resource limits for sandboxed Python execution.
///
/// All fields are optional — pass `null` for any field to leave that limit
/// disabled (or at the default). Construct in Kotlin as a data class:
///
/// ```kotlin
/// val limits = MontyLimits(maxDurationSecs = 5.0, maxRecursionDepth = 100u)
/// monty.run("{}", handler, limits)
/// ```
///
/// The available limits mirror `pydantic_monty.ResourceLimits`:
/// - `max_allocations` — heap allocation count before `MemoryError`
/// - `max_duration_secs` — wall-clock seconds before `TimeoutError`
/// - `max_memory` — heap bytes before `MemoryError`
/// - `gc_interval` — run GC every N allocations
/// - `max_recursion_depth` — call stack depth before `RecursionError`
#[derive(Debug, Clone, uniffi::Record)]
pub struct MontyLimits {
    /// Maximum number of heap allocations before raising `MemoryError`. `null` = no limit.
    pub max_allocations: Option<u64>,
    /// Maximum wall-clock execution time in seconds before raising `TimeoutError`. `null` = no limit.
    pub max_duration_secs: Option<f64>,
    /// Maximum heap memory in bytes before raising `MemoryError`. `null` = no limit.
    pub max_memory: Option<u64>,
    /// Run garbage collection every N allocations. `null` = GC disabled.
    pub gc_interval: Option<u64>,
    /// Maximum call stack depth before raising `RecursionError`. `null` = default (1000).
    pub max_recursion_depth: Option<u64>,
}

impl From<MontyLimits> for ResourceLimits {
    fn from(l: MontyLimits) -> Self {
        let mut limits = ResourceLimits::new();
        if let Some(n) = l.max_allocations {
            limits = limits.max_allocations(n as usize);
        }
        if let Some(s) = l.max_duration_secs {
            limits = limits.max_duration(Duration::from_secs_f64(s));
        }
        if let Some(m) = l.max_memory {
            limits = limits.max_memory(m as usize);
        }
        if let Some(g) = l.gc_interval {
            limits = limits.gc_interval(g as usize);
        }
        if let Some(d) = l.max_recursion_depth {
            limits = limits.max_recursion_depth(Some(d as usize));
        }
        limits
    }
}

// =============================================================================
// Callback interfaces
// =============================================================================

/// Kotlin-implemented handler for external function calls from sandboxed Python.
///
/// When Python code inside the sandbox calls a function declared as external
/// (e.g., `fetch(url)`), Monty pauses execution and dispatches to this handler.
/// The handler must return a JSON-encoded value that becomes the Python function's
/// return value, or throw a `MontyError` to propagate an error into the sandbox.
///
/// ## JSON encoding
///
/// - `args_json`: a JSON array of positional arguments (e.g., `[1, "hello"]`)
/// - `kwargs_json`: a JSON object of keyword arguments (e.g., `{"timeout": 30}`)
/// - return value: any JSON value that will be converted back to a Python object
///
/// ## Example (Kotlin)
///
/// ```kotlin
/// val handler = object : ExternalFunctionHandler {
///     override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
///         // Double the first positional argument
///         val x = argsJson.trim('[', ']').toInt()
///         return (x * 2).toString()
///     }
/// }
/// ```
#[uniffi::export(callback_interface)]
pub trait ExternalFunctionHandler: Send + Sync {
    /// Called when Python code invokes an external function.
    ///
    /// # Arguments
    /// * `function_name` – the name of the Python function being called
    /// * `args_json` – positional arguments as a JSON array string
    /// * `kwargs_json` – keyword arguments as a JSON object string
    ///
    /// # Returns
    /// A JSON string representing the return value, e.g. `"42"`, `"null"`, `"\"hello\""`.
    ///
    /// # Errors
    /// Return a `MontyError` to propagate an error into the Python sandbox.
    fn call(&self, function_name: String, args_json: String, kwargs_json: String) -> Result<String, MontyError>;
}

// =============================================================================
// Concurrent external function types
// =============================================================================

/// A single external function call that has been deferred for concurrent resolution.
///
/// Produced by `run_concurrent()` when a Kotlin handler returns `null` from its
/// `call()` method, signalling that the result will be provided later via
/// `resolve_pending()`. All fields are JSON-encoded for easy handling in Kotlin.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PendingFunctionCall {
    /// Unique identifier for this call, used to correlate results in `resolve_pending()`.
    pub call_id: u32,
    /// The name of the Python external function being called.
    pub function_name: String,
    /// Positional arguments as a JSON array string (e.g. `"[1, \"hello\"]"`).
    pub args_json: String,
    /// Keyword arguments as a JSON object string (e.g. `"{\"timeout\": 30}"`).
    pub kwargs_json: String,
}

/// The resolved result of a single pending external function call.
///
/// Return one `FunctionCallResult` per `PendingFunctionCall` from `resolve_pending()`.
/// The `call_id` must match a `PendingFunctionCall.call_id` in the current batch.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FunctionCallResult {
    /// The call identifier this result belongs to — must match a `PendingFunctionCall.call_id`.
    pub call_id: u32,
    /// The JSON-encoded return value (e.g. `"42"`, `"null"`, `"\"hello\""`).
    pub result_json: String,
}

/// Kotlin-implemented handler for concurrent external function calls.
///
/// Used with `MontyKt.runConcurrent()` to support Python code that uses
/// `asyncio.gather()` to execute multiple external function calls concurrently.
///
/// ## How it works
///
/// For each `FunctionCall` Monty yields, the handler's `call()` method is invoked:
/// - Return a JSON string → the call is resolved **synchronously** and execution continues.
/// - Return `null` → the call is marked as a **pending future**; execution continues
///   until Python tries to `await` the result, at which point Monty yields
///   `ResolveFutures` and the binding calls `resolve_pending()` with all queued calls.
///
/// ## Concurrent resolution
///
/// In `resolve_pending()`, the Kotlin host receives all currently-pending calls in one
/// batch and may execute them on JVM threads concurrently (e.g. using coroutines,
/// virtual threads, or a thread pool) before returning the results.
///
/// ## Example (Kotlin)
///
/// ```kotlin
/// val handler = object : ConcurrentFunctionHandler {
///     // Return null to defer all calls; resolve concurrently in resolvePending
///     override fun call(callId: UInt, functionName: String, argsJson: String, kwargsJson: String): String? = null
///
///     override fun resolvePending(calls: List<PendingFunctionCall>): List<FunctionCallResult> {
///         // Execute all pending calls concurrently using virtual threads
///         return calls.parallelStream().map { call ->
///             FunctionCallResult(callId = call.callId, resultJson = fetchData(call))
///         }.toList()
///     }
/// }
/// ```
#[uniffi::export(callback_interface)]
pub trait ConcurrentFunctionHandler: Send + Sync {
    /// Called for each external function invocation.
    ///
    /// Return a JSON string to resolve the call synchronously (result injected immediately),
    /// or return `null` to defer it — the call will be queued and delivered to
    /// `resolve_pending()` when Monty pauses for future resolution.
    ///
    /// # Errors
    /// Throw a `MontyError` (or any exception, converted via UniFFI) to propagate an
    /// error into the Python sandbox for this call.
    fn call(
        &self,
        call_id: u32,
        function_name: String,
        args_json: String,
        kwargs_json: String,
    ) -> Result<Option<String>, MontyError>;

    /// Called when Monty is waiting for all deferred (future) calls to be resolved.
    ///
    /// Receives a batch of previously-deferred `PendingFunctionCall`s and must return a
    /// `FunctionCallResult` for each one (in any order). The host may execute them
    /// concurrently before returning.
    ///
    /// # Errors
    /// Throw a `MontyError` to abort execution with a runtime error.
    fn resolve_pending(&self, calls: Vec<PendingFunctionCall>) -> Result<Vec<FunctionCallResult>, MontyError>;
}

// =============================================================================
// Main interpreter object
// =============================================================================

/// A compiled Monty interpreter instance ready to execute sandboxed Python code.
///
/// Parses and compiles Python code once on creation (via `create()`), then can
/// be run multiple times with different inputs. The parsed code is stored in the
/// `MontyRun` so re-parsing is not needed on each `run()` call.
///
/// ## Thread safety
///
/// `MontyKt` is `Send + Sync` because `MontyRun` is cloned before each execution
/// (required because `start()` consumes the runner). Multiple threads may share
/// the same `MontyKt` and call `run()` concurrently.
#[derive(uniffi::Object)]
pub struct MontyKt {
    /// Compiled Python code runner, cloned before each execution.
    runner: MontyRun,
    /// Ordered list of input variable names, used to extract values from the inputs JSON.
    input_names: Vec<String>,
}

#[uniffi::export]
impl MontyKt {
    /// Parses and optionally type-checks the given Python code, returning a
    /// ready-to-run instance.
    ///
    /// # Arguments
    /// * `code` – Python source code to execute
    /// * `script_name` – name used in tracebacks; defaults to `"main.py"`
    /// * `inputs` – input variable names available in the code
    /// * `external_functions` – names of external functions the code may call
    /// * `type_check_prefix` – if `Some`, run static type checking before compiling.
    ///   The string is prepended to `code` as type stubs (pass `""` to type-check
    ///   with no stubs). Pass `null` to skip type checking entirely.
    ///
    /// # Errors
    /// - `MontyError::SyntaxError` if the code cannot be parsed
    /// - `MontyError::TypingError` if type checking is enabled and finds errors
    #[uniffi::constructor]
    pub fn create(
        code: String,
        script_name: Option<String>,
        inputs: Option<Vec<String>>,
        external_functions: Option<Vec<String>>,
        type_check_prefix: Option<String>,
    ) -> Result<Arc<Self>, MontyError> {
        let script_name = script_name.unwrap_or_else(|| "main.py".to_string());
        let input_names = inputs.unwrap_or_default();
        let external_function_names = external_functions.unwrap_or_default();

        // Run static type checking if requested.
        if let Some(ref prefix) = type_check_prefix {
            run_type_check(&code, &script_name, prefix)?;
        }

        let runner =
            MontyRun::new(code, &script_name, input_names.clone(), external_function_names).map_err(|exc| {
                MontyError::SyntaxError {
                    reason: exc.to_string(),
                }
            })?;

        Ok(Arc::new(Self { runner, input_names }))
    }

    /// Executes the compiled Python code with the given inputs.
    ///
    /// Drives the full execution loop, dispatching each external function call to
    /// `handler` and feeding the JSON-encoded return values back into the sandbox.
    ///
    /// # Arguments
    /// * `inputs_json` – JSON object mapping input names to their values,
    ///   e.g. `{"x": 10, "name": "Alice"}`. Use `"{}"` when there are no inputs.
    /// * `handler` – called for each external function invocation
    /// * `limits` – optional resource limits (allocations, time, memory, recursion depth).
    ///   Pass `null` for unlimited execution.
    ///
    /// # Returns
    /// A JSON string representing the final return value of the executed code.
    ///
    /// # Errors
    /// - `MontyError::RuntimeError` for Python runtime exceptions (including
    ///   `MemoryError`, `TimeoutError`, `RecursionError` when limits are hit)
    /// - `MontyError::OsCallNotSupported` if the code attempts an OS-level call
    pub fn run(
        &self,
        inputs_json: String,
        handler: Box<dyn ExternalFunctionHandler>,
        limits: Option<MontyLimits>,
    ) -> Result<String, MontyError> {
        let input_values = parse_inputs_json(&inputs_json, &self.input_names)?;

        // Clone runner since start() consumes it, allowing this MontyKt to be reused.
        let runner = self.runner.clone();
        let mut print_writer = PrintWriter::Stdout;

        match limits {
            Some(l) => run_execution_loop(runner, input_values, LimitedTracker::new(l.into()), &*handler, &mut print_writer),
            None => run_execution_loop(runner, input_values, NoLimitTracker, &*handler, &mut print_writer),
        }
    }

    /// Executes the compiled Python code with support for concurrent external function calls.
    ///
    /// Use this method instead of `run()` when the Python code uses `asyncio.gather()` or
    /// `await` to call external functions concurrently. The `handler` decides per-call
    /// whether to resolve synchronously (return a JSON string) or defer to the batch
    /// `resolve_pending()` callback, which the JVM host can execute concurrently.
    ///
    /// ## Sequential calls
    ///
    /// For `result = sync_func()` (no `await`), the handler's `call()` should return
    /// a JSON string immediately so the value is available synchronously.
    ///
    /// ## Concurrent calls
    ///
    /// For `await asyncio.gather(f1(), f2(), f3())`, the handler's `call()` should return
    /// `null` for each invocation. Once all three are pending, Monty yields and the binding
    /// calls `resolve_pending([f1_call, f2_call, f3_call])`, which the host resolves
    /// concurrently before returning the results.
    ///
    /// # Arguments
    /// * `inputs_json` – JSON object of input values, e.g. `"{\"x\": 42}"`
    /// * `handler` – per-call dispatch and batch resolution callback
    /// * `limits` – optional resource limits; `null` for unlimited execution
    ///
    /// # Returns
    /// A JSON string representing the final value of the executed code.
    ///
    /// # Errors
    /// - `MontyError::RuntimeError` for Python runtime exceptions or handler errors
    /// - `MontyError::OsCallNotSupported` if the code attempts an OS-level call
    pub fn run_concurrent(
        &self,
        inputs_json: String,
        handler: Box<dyn ConcurrentFunctionHandler>,
        limits: Option<MontyLimits>,
    ) -> Result<String, MontyError> {
        let input_values = parse_inputs_json(&inputs_json, &self.input_names)?;

        // Clone runner since start() consumes it, allowing this MontyKt to be reused.
        let runner = self.runner.clone();
        let mut print_writer = PrintWriter::Stdout;

        match limits {
            Some(l) => run_concurrent_loop(runner, input_values, LimitedTracker::new(l.into()), &*handler, &mut print_writer),
            None => run_concurrent_loop(runner, input_values, NoLimitTracker, &*handler, &mut print_writer),
        }
    }
}

/// Drives the Monty execution loop with the given tracker.
///
/// Extracted into a generic helper so `run()` can dispatch to either
/// `NoLimitTracker` or `LimitedTracker` without duplicating the loop logic.
/// The tracker type affects the `Snapshot<T>` stored inside `RunProgress::FunctionCall`,
/// requiring this function to be generic over `T`.
fn run_execution_loop(
    runner: MontyRun,
    input_values: Vec<MontyObject>,
    tracker: impl ResourceTracker,
    handler: &dyn ExternalFunctionHandler,
    print_writer: &mut PrintWriter,
) -> Result<String, MontyError> {
    let mut progress = runner.start(input_values, tracker, print_writer).map_err(runtime_error)?;

    loop {
        match progress {
            RunProgress::Complete(result) => {
                return Ok(monty_to_json(&result));
            }
            RunProgress::FunctionCall { function_name, args, kwargs, state, .. } => {
                let args_json = args_to_json(&args);
                let kwargs_json = kwargs_to_json(&kwargs);

                // Call the Kotlin handler; any error becomes a RuntimeError.
                let return_json = handler
                    .call(function_name, args_json, kwargs_json)
                    .map_err(|e| MontyError::RuntimeError { reason: e.to_string() })?;

                let return_value = json_to_monty(&return_json)?;

                progress = state.run(ExternalResult::Return(return_value), print_writer).map_err(runtime_error)?;
            }
            RunProgress::OsCall { function, .. } => {
                return Err(MontyError::OsCallNotSupported { reason: format!("{function:?}") });
            }
            RunProgress::ResolveFutures(_) => {
                return Err(MontyError::OsCallNotSupported {
                    reason: "Async futures (ResolveFutures) are not supported in the Kotlin binding".to_string(),
                });
            }
        }
    }
}

/// Drives the Monty execution loop with support for concurrent future resolution.
///
/// Similar to `run_execution_loop`, but for each `FunctionCall` it consults the
/// `ConcurrentFunctionHandler`:
/// - If `call()` returns `Some(json)` → resolve immediately (synchronous).
/// - If `call()` returns `None` → mark the call as a pending future and continue;
///   when Monty yields `ResolveFutures`, the batch is sent to `resolve_pending()`.
///
/// This allows Python code using `asyncio.gather()` to trigger concurrent resolution
/// on the JVM side: all pending futures from one gather are delivered together to
/// `resolve_pending()`, where the Kotlin host can dispatch them on JVM threads.
fn run_concurrent_loop(
    runner: MontyRun,
    input_values: Vec<MontyObject>,
    tracker: impl ResourceTracker,
    handler: &dyn ConcurrentFunctionHandler,
    print_writer: &mut PrintWriter,
) -> Result<String, MontyError> {
    let mut progress = runner.start(input_values, tracker, print_writer).map_err(runtime_error)?;
    // Calls deferred via run_pending(), awaiting batch resolution.
    let mut pending: Vec<PendingFunctionCall> = Vec::new();

    loop {
        match progress {
            RunProgress::Complete(result) => return Ok(monty_to_json(&result)),
            RunProgress::FunctionCall { call_id, function_name, args, kwargs, state, .. } => {
                let args_json = args_to_json(&args);
                let kwargs_json = kwargs_to_json(&kwargs);

                // Ask the handler: sync resolve (Some) or defer (None)?
                let maybe_result = handler.call(call_id, function_name.clone(), args_json.clone(), kwargs_json.clone())?;

                progress = match maybe_result {
                    Some(return_json) => {
                        // Resolved synchronously — feed the result back immediately.
                        let value = json_to_monty(&return_json)?;
                        state.run(ExternalResult::Return(value), print_writer).map_err(runtime_error)?
                    }
                    None => {
                        // Deferred — register as a pending future and continue execution.
                        pending.push(PendingFunctionCall { call_id, function_name, args_json, kwargs_json });
                        state.run_pending(print_writer).map_err(runtime_error)?
                    }
                };
            }
            RunProgress::OsCall { function, .. } => {
                return Err(MontyError::OsCallNotSupported { reason: format!("{function:?}") });
            }
            RunProgress::ResolveFutures(future_snapshot) => {
                // Determine which pending calls Monty is currently blocked on.
                let needed: std::collections::HashSet<u32> =
                    future_snapshot.pending_call_ids().iter().copied().collect();

                // Extract only the relevant pending calls; keep the rest for later rounds.
                let (to_resolve, remaining): (Vec<_>, Vec<_>) =
                    pending.drain(..).partition(|c| needed.contains(&c.call_id));
                pending = remaining;

                // Deliver the batch to the Kotlin host for (potentially concurrent) resolution.
                let results = handler.resolve_pending(to_resolve)?;

                let external_results = results
                    .into_iter()
                    .map(|r| json_to_monty(&r.result_json).map(|v| (r.call_id, ExternalResult::Return(v))))
                    .collect::<Result<Vec<_>, _>>()?;

                progress = future_snapshot.resume(external_results, print_writer).map_err(runtime_error)?;
            }
        }
    }
}

// =============================================================================
// Type checking
// =============================================================================

/// Runs static type checking on `code`, optionally using `prefix` as a stub file.
///
/// When `prefix` is non-empty it is written as a `.pyi` stub file so that
/// declarations like `def add(a: int, b: int) -> int: ...` are legal (stub files
/// permit `...`-bodied functions with non-`None` return types). The type checker
/// then generates `from stubs import *` in the main file so the stubs are visible.
///
/// When `prefix` is empty, only the main source is checked with no extra stubs.
///
/// Returns `Err(MontyError::TypingError)` when diagnostics are found, or
/// `Err(MontyError::RuntimeError)` if the type checker itself fails unexpectedly.
fn run_type_check(code: &str, script_name: &str, prefix: &str) -> Result<(), MontyError> {
    let source_file = SourceFile::new(code, script_name);

    let stubs_file = if prefix.is_empty() {
        None
    } else {
        Some(SourceFile::new(prefix, "stubs.pyi"))
    };

    match type_check(&source_file, stubs_file.as_ref()) {
        Ok(None) => Ok(()),
        Ok(Some(diagnostics)) => Err(MontyError::TypingError {
            reason: diagnostics.to_string(),
        }),
        Err(e) => Err(MontyError::RuntimeError {
            reason: format!("Type checker internal error: {e}"),
        }),
    }
}

// =============================================================================
// JSON conversion helpers
// =============================================================================

/// Converts a `MontyObject` to a JSON string.
///
/// The mapping follows natural Python-to-JSON semantics:
/// - `None` → `"null"`
/// - `Bool` → `"true"` / `"false"`
/// - `Int` / `Float` → number
/// - `String` → quoted string
/// - `List` / `Tuple` → JSON array
/// - `Dict` → JSON object (string keys only; non-string keys are skipped)
/// - All other types (e.g. `Ellipsis`, `Type`, `Dataclass`) → `"null"`
fn monty_to_json(obj: &MontyObject) -> String {
    monty_to_json_value(obj).to_string()
}

/// Recursively converts a `MontyObject` to a `serde_json::Value`.
fn monty_to_json_value(obj: &MontyObject) -> JsonValue {
    match obj {
        MontyObject::None => JsonValue::Null,
        MontyObject::Bool(b) => JsonValue::Bool(*b),
        MontyObject::Int(i) => JsonValue::Number((*i).into()),
        MontyObject::Float(f) => serde_json::Number::from_f64(*f)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        MontyObject::String(s) => JsonValue::String(s.clone()),
        MontyObject::List(items) | MontyObject::Tuple(items) => {
            JsonValue::Array(items.iter().map(monty_to_json_value).collect())
        }
        MontyObject::NamedTuple { values, .. } => JsonValue::Array(values.iter().map(monty_to_json_value).collect()),
        MontyObject::Dict(pairs) => {
            let mut map = serde_json::Map::new();
            for (k, v) in pairs {
                if let MontyObject::String(key) = k {
                    map.insert(key.clone(), monty_to_json_value(v));
                }
                // Non-string keys are not representable in JSON; skip them.
            }
            JsonValue::Object(map)
        }
        // Types without JSON equivalents → null
        MontyObject::BigInt(_)
        | MontyObject::Bytes(_)
        | MontyObject::Set(_)
        | MontyObject::FrozenSet(_)
        | MontyObject::Ellipsis
        | MontyObject::Exception { .. }
        | MontyObject::Type(_)
        | MontyObject::BuiltinFunction(_)
        | MontyObject::Dataclass { .. }
        | MontyObject::Path(_)
        | MontyObject::Repr(_)
        | MontyObject::Cycle(_, _) => JsonValue::Null,
    }
}

/// Converts a JSON string to a `MontyObject`.
///
/// The mapping is:
/// - `null` → `None`
/// - `true` / `false` → `Bool`
/// - integer number → `Int`
/// - float number → `Float`
/// - string → `String`
/// - array → `List`
/// - object → `Dict` (keys become `MontyObject::String`)
///
/// # Errors
/// Returns `MontyError::RuntimeError` if the string is not valid JSON.
fn json_to_monty(s: &str) -> Result<MontyObject, MontyError> {
    let value: JsonValue = serde_json::from_str(s).map_err(|e| MontyError::RuntimeError {
        reason: format!("Invalid JSON from external function handler: {e}"),
    })?;
    Ok(json_value_to_monty(value))
}

/// Recursively converts a `serde_json::Value` to a `MontyObject`.
fn json_value_to_monty(value: JsonValue) -> MontyObject {
    match value {
        JsonValue::Null => MontyObject::None,
        JsonValue::Bool(b) => MontyObject::Bool(b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                MontyObject::Int(i)
            } else if let Some(f) = n.as_f64() {
                MontyObject::Float(f)
            } else {
                // Fallback: should not happen with standard JSON numbers.
                MontyObject::None
            }
        }
        JsonValue::String(s) => MontyObject::String(s),
        JsonValue::Array(items) => MontyObject::List(items.into_iter().map(json_value_to_monty).collect()),
        JsonValue::Object(map) => {
            let pairs: Vec<(MontyObject, MontyObject)> = map
                .into_iter()
                .map(|(k, v)| (MontyObject::String(k), json_value_to_monty(v)))
                .collect();
            MontyObject::dict(pairs)
        }
    }
}

/// Serializes positional arguments to a JSON array string.
fn args_to_json(args: &[MontyObject]) -> String {
    JsonValue::Array(args.iter().map(monty_to_json_value).collect()).to_string()
}

/// Serializes keyword arguments to a JSON object string.
///
/// Only string keys are included; non-string keys are silently skipped.
fn kwargs_to_json(kwargs: &[(MontyObject, MontyObject)]) -> String {
    let mut map = serde_json::Map::new();
    for (k, v) in kwargs {
        if let MontyObject::String(key) = k {
            map.insert(key.clone(), monty_to_json_value(v));
        }
    }
    JsonValue::Object(map).to_string()
}

/// Parses a JSON object string into an ordered list of `MontyObject` input values.
///
/// Values are extracted in the order of `input_names`, matching the contract
/// of `MontyRun::start()`.
///
/// # Errors
/// Returns `MontyError::RuntimeError` if the JSON is invalid or a required input
/// is missing.
fn parse_inputs_json(inputs_json: &str, input_names: &[String]) -> Result<Vec<MontyObject>, MontyError> {
    if input_names.is_empty() {
        return Ok(vec![]);
    }

    let parsed: JsonValue = serde_json::from_str(inputs_json).map_err(|e| MontyError::RuntimeError {
        reason: format!("Invalid inputs JSON: {e}"),
    })?;

    let obj = parsed.as_object().ok_or_else(|| MontyError::RuntimeError {
        reason: "inputs_json must be a JSON object".to_string(),
    })?;

    input_names
        .iter()
        .map(|name| {
            obj.get(name)
                .map(|v| json_value_to_monty(v.clone()))
                .ok_or_else(|| MontyError::RuntimeError {
                    reason: format!("Missing required input: '{name}'"),
                })
        })
        .collect()
}

/// Converts a `MontyException` to a `MontyError::RuntimeError`.
fn runtime_error(exc: MontyException) -> MontyError {
    MontyError::RuntimeError {
        reason: exc.to_string(),
    }
}
