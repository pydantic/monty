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

use monty::{ExternalResult, MontyException, MontyObject, MontyRun, NoLimitTracker, PrintWriter, RunProgress};
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
// Callback interface
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
    ///
    /// # Returns
    /// A JSON string representing the final return value of the executed code.
    ///
    /// # Errors
    /// - `MontyError::RuntimeError` for Python runtime exceptions
    /// - `MontyError::OsCallNotSupported` if the code attempts an OS-level call
    pub fn run(&self, inputs_json: String, handler: Box<dyn ExternalFunctionHandler>) -> Result<String, MontyError> {
        let input_values = parse_inputs_json(&inputs_json, &self.input_names)?;

        // Clone runner since start() consumes it, allowing this MontyKt to be reused.
        let runner = self.runner.clone();

        let mut print_writer = PrintWriter::Stdout;
        let mut progress = runner
            .start(input_values, NoLimitTracker, &mut print_writer)
            .map_err(runtime_error)?;

        loop {
            match progress {
                RunProgress::Complete(result) => {
                    return Ok(monty_to_json(&result));
                }
                RunProgress::FunctionCall {
                    function_name,
                    args,
                    kwargs,
                    state,
                    ..
                } => {
                    let args_json = args_to_json(&args);
                    let kwargs_json = kwargs_to_json(&kwargs);

                    // Call the Kotlin handler; any error becomes a RuntimeError.
                    let return_json = handler
                        .call(function_name, args_json, kwargs_json)
                        .map_err(|e| MontyError::RuntimeError { reason: e.to_string() })?;

                    let return_value = json_to_monty(&return_json)?;

                    progress = state
                        .run(ExternalResult::Return(return_value), &mut print_writer)
                        .map_err(runtime_error)?;
                }
                RunProgress::OsCall { function, .. } => {
                    return Err(MontyError::OsCallNotSupported {
                        reason: format!("{function:?}"),
                    });
                }
                RunProgress::ResolveFutures(_) => {
                    return Err(MontyError::OsCallNotSupported {
                        reason: "Async futures (ResolveFutures) are not supported in the Kotlin binding".to_string(),
                    });
                }
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
