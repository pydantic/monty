//! The main `Monty` class and iterative execution support for the TypeScript/JavaScript bindings.
//!
//! Provides a sandboxed Python interpreter that can be configured with inputs,
//! external functions, and resource limits. Supports both immediate execution
//! via `run()` and iterative execution via `start()`/`resume()`.
//!
//! ## Quick Start
//!
//! ```typescript
//! import { Monty } from 'monty';
//!
//! // Simple execution
//! const m = new Monty('1 + 2');
//! const result = m.run(); // returns 3
//!
//! // With inputs
//! const m2 = new Monty('x + y', { inputs: ['x', 'y'] });
//! const result2 = m2.run({ inputs: { x: 10, y: 20 } }); // returns 30
//! ```
//!
//! ## Iterative Execution
//!
//! ```text
//! Monty.start() -> MontySnapshot | MontyComplete
//!                       |
//!                       v
//! MontySnapshot.resume() -> MontySnapshot | MontyComplete
//!                                |
//!                                v
//!                          (repeat until complete)
//! ```
//!
//! ```typescript
//! const m = new Monty('result = external_func(1, 2)', {
//!   externalFunctions: ['external_func']
//! });
//!
//! let progress = m.start();
//! while (progress instanceof MontySnapshot) {
//!   console.log(`Calling ${progress.functionName} with args:`, progress.args);
//!   progress = progress.resume({ returnValue: 42 });
//! }
//! console.log('Final result:', progress.output);
//! ```

use std::borrow::Cow;

use monty::{
    CollectStringPrint, ExcType, ExternalResult, LimitedTracker, MontyException, MontyObject, MontyRun, NoLimitTracker,
    ResourceTracker, RunProgress, Snapshot,
};
use monty_type_checking::type_check;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::Value;

use crate::{
    convert::{monty_to_js, monty_to_serde, serde_to_monty, JsMontyObject},
    exceptions::{monty_exception_to_error, typing_failure_to_error},
    limits::JsResourceLimits,
};

// =============================================================================
// Monty - Main interpreter class
// =============================================================================

/// A sandboxed Python interpreter instance.
///
/// Parses and compiles Python code on initialization, then can be run
/// multiple times with different input values. This separates the parsing
/// cost from execution, making repeated runs more efficient.
#[napi]
pub struct Monty {
    /// The compiled code runner, ready to execute.
    runner: MontyRun,
    /// The artificial name of the python code "file".
    script_name: String,
    /// Names of input variables expected by the code.
    input_names: Vec<String>,
    /// Names of external functions the code can call.
    external_function_names: Vec<String>,
}

/// Options for creating a new Monty instance.
#[napi(object)]
pub struct MontyOptions {
    /// Name used in tracebacks and error messages. Default: 'main.py'
    pub script_name: Option<String>,
    /// List of input variable names available in the code.
    pub inputs: Option<Vec<String>>,
    /// List of external function names the code can call.
    pub external_functions: Option<Vec<String>>,
    /// Whether to perform type checking on the code. Default: false
    pub type_check: Option<bool>,
    /// Optional code to prepend before type checking.
    pub type_check_prefix_code: Option<String>,
}

/// Options for running code.
#[napi(object)]
pub struct RunOptions {
    /// Dict of input variable values as a JSON object.
    /// Keys are input names, values are the input values.
    pub inputs: Option<Value>,
    /// Resource limits configuration.
    pub limits: Option<JsResourceLimits>,
}

/// Options for starting execution.
#[napi(object)]
pub struct StartOptions {
    /// Dict of input variable values as a JSON object.
    pub inputs: Option<Value>,
    /// Resource limits configuration.
    pub limits: Option<JsResourceLimits>,
}

#[napi]
impl Monty {
    /// Creates a new Monty interpreter by parsing the given code.
    ///
    /// @param code - Python code to execute
    /// @param options - Configuration options
    #[napi(constructor)]
    pub fn new(code: String, options: Option<MontyOptions>) -> Result<Self> {
        let options = options.unwrap_or(MontyOptions {
            script_name: None,
            inputs: None,
            external_functions: None,
            type_check: None,
            type_check_prefix_code: None,
        });

        let script_name = options.script_name.unwrap_or_else(|| "main.py".to_string());
        let input_names = options.inputs.unwrap_or_default();
        let external_function_names = options.external_functions.unwrap_or_default();
        let do_type_check = options.type_check.unwrap_or(false);

        // Perform type checking if requested
        if do_type_check {
            run_type_check(&code, &script_name, options.type_check_prefix_code.as_deref())?;
        }

        // Create the runner (parses the code)
        let runner = MontyRun::new(code, &script_name, input_names.clone(), external_function_names.clone())
            .map_err(|e| monty_exception_to_error(&e))?;

        Ok(Self {
            runner,
            script_name,
            input_names,
            external_function_names,
        })
    }

    /// Performs static type checking on the code.
    ///
    /// Analyzes the code for type errors without executing it.
    ///
    /// @param prefixCode - Optional code to prepend before type checking
    #[napi]
    pub fn type_check(&self, prefix_code: Option<String>) -> Result<()> {
        run_type_check(self.runner.code(), &self.script_name, prefix_code.as_deref())
    }

    /// Executes the code and returns the result.
    ///
    /// @param options - Execution options (inputs, limits)
    /// @returns The result of the last expression in the code as JSON
    #[napi]
    pub fn run<'env>(&self, env: &'env Env, options: Option<RunOptions>) -> Result<JsMontyObject<'env>> {
        let options = options.unwrap_or(RunOptions {
            inputs: None,
            limits: None,
        });

        // Extract input values
        let input_values = self.extract_input_values(options.inputs.as_ref())?;

        // Run with appropriate tracker
        let mut print_output = CollectStringPrint::default();

        let result = if let Some(limits) = options.limits {
            let tracker = LimitedTracker::new(limits.into());
            self.runner.run(input_values, tracker, &mut print_output)
        } else {
            self.runner.run(input_values, NoLimitTracker, &mut print_output)
        };

        match result {
            Ok(value) => monty_to_js(&value, env),
            Err(exc) => Err(monty_exception_to_error(&exc)),
        }
    }

    /// Starts execution and returns either a snapshot (paused at external call) or completion.
    ///
    /// This method enables iterative execution where code pauses at external function
    /// calls, allowing the host to provide return values or exceptions before resuming.
    ///
    /// @param options - Execution options (inputs, limits)
    /// @returns MontySnapshot if an external function call is pending, MontyComplete if done
    #[napi]
    pub fn start(&self, options: Option<StartOptions>) -> Result<Either<MontySnapshot, MontyComplete>> {
        let options = options.unwrap_or(StartOptions {
            inputs: None,
            limits: None,
        });

        // Extract input values
        let input_values = self.extract_input_values(options.inputs.as_ref())?;

        // Clone the runner since start() consumes it - allows reuse of the parsed code
        let runner = self.runner.clone();
        let mut print_output = CollectStringPrint::default();

        // Start execution with appropriate tracker
        if let Some(limits) = options.limits {
            let tracker = LimitedTracker::new(limits.into());
            let progress = runner
                .start(input_values, tracker, &mut print_output)
                .map_err(|e| monty_exception_to_error(&e))?;
            Ok(progress_to_result(progress, self.script_name.clone()))
        } else {
            let progress = runner
                .start(input_values, NoLimitTracker, &mut print_output)
                .map_err(|e| monty_exception_to_error(&e))?;
            Ok(progress_to_result(progress, self.script_name.clone()))
        }
    }

    /// Serializes the Monty instance to a binary format.
    ///
    /// The serialized data can be stored and later restored with `Monty.load()`.
    /// This allows caching parsed code to avoid re-parsing on subsequent runs.
    ///
    /// @returns Buffer containing the serialized Monty instance
    #[napi]
    pub fn dump(&self) -> Result<Buffer> {
        let serialized = SerializedMonty {
            runner: self.runner.clone(),
            script_name: self.script_name.clone(),
            input_names: self.input_names.clone(),
            external_function_names: self.external_function_names.clone(),
        };
        let bytes =
            postcard::to_allocvec(&serialized).map_err(|e| Error::from_reason(format!("Serialization failed: {e}")))?;
        Ok(Buffer::from(bytes))
    }

    /// Deserializes a Monty instance from binary format.
    ///
    /// @param data - The serialized Monty data from `dump()`
    /// @returns A new Monty instance
    #[napi(factory)]
    pub fn load(data: Buffer) -> Result<Self> {
        let serialized: SerializedMonty =
            postcard::from_bytes(&data).map_err(|e| Error::from_reason(format!("Deserialization failed: {e}")))?;

        Ok(Self {
            runner: serialized.runner,
            script_name: serialized.script_name,
            input_names: serialized.input_names,
            external_function_names: serialized.external_function_names,
        })
    }

    /// Returns the script name.
    #[napi(getter)]
    pub fn script_name(&self) -> String {
        self.script_name.clone()
    }

    /// Returns the input variable names.
    #[napi(getter)]
    pub fn inputs(&self) -> Vec<String> {
        self.input_names.clone()
    }

    /// Returns the external function names.
    #[napi(getter)]
    pub fn external_functions(&self) -> Vec<String> {
        self.external_function_names.clone()
    }

    /// Returns a string representation of the Monty instance.
    #[napi]
    pub fn repr(&self) -> String {
        use std::fmt::Write;
        let lines = self.runner.code().lines().count();
        let mut s = format!(
            "Monty(<{} line{} of code>, scriptName='{}'",
            lines,
            if lines == 1 { "" } else { "s" },
            self.script_name
        );
        if !self.input_names.is_empty() {
            write!(s, ", inputs={:?}", self.input_names).unwrap();
        }
        if !self.external_function_names.is_empty() {
            write!(s, ", externalFunctions={:?}", self.external_function_names).unwrap();
        }
        s.push(')');
        s
    }

    /// Extracts input values from the JSON Value in the order they were declared.
    fn extract_input_values(&self, inputs: Option<&Value>) -> Result<Vec<MontyObject>> {
        if self.input_names.is_empty() {
            if inputs.is_some() {
                return Err(Error::from_reason(
                    "No input variables declared but inputs object was provided",
                ));
            }
            return Ok(vec![]);
        }

        let Some(inputs) = inputs else {
            return Err(Error::from_reason(format!(
                "Missing required inputs: {:?}",
                self.input_names
            )));
        };

        let inputs_obj = inputs
            .as_object()
            .ok_or_else(|| Error::from_reason("inputs must be an object"))?;

        // Extract values in declaration order
        self.input_names
            .iter()
            .map(|name| {
                let value = inputs_obj
                    .get(name)
                    .ok_or_else(|| Error::from_reason(format!("Missing required input: '{name}'")))?;
                serde_to_monty(value)
            })
            .collect()
    }
}

/// Performs type checking on the code.
fn run_type_check(code: &str, script_name: &str, prefix_code: Option<&str>) -> Result<()> {
    let source_code: Cow<str> = if let Some(prefix_code) = prefix_code {
        format!("{prefix_code}\n{code}").into()
    } else {
        code.into()
    };

    let result =
        type_check(&source_code, script_name).map_err(|e| Error::from_reason(format!("Type checking failed: {e}")))?;

    if let Some(failure) = result {
        Err(typing_failure_to_error(failure))
    } else {
        Ok(())
    }
}

// =============================================================================
// EitherSnapshot - Internal enum to handle generic resource tracker types
// =============================================================================

/// Runtime execution snapshot, holds multiple resource tracker types since napi structs can't be generic.
///
/// Used internally by `MontySnapshot` to store execution state.
/// The `Done` variant indicates the snapshot has been consumed.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum EitherSnapshot {
    NoLimit(Snapshot<NoLimitTracker>),
    Limited(Snapshot<LimitedTracker>),
    /// Done is used when taking the snapshot to run it.
    /// Should only be set after execution is complete.
    Done,
}

// =============================================================================
// MontySnapshot - Paused execution at an external function call
// =============================================================================

/// Represents paused execution waiting for an external function call return value.
///
/// Contains information about the pending external function call and allows
/// resuming execution with the return value or an exception.
#[napi]
pub struct MontySnapshot {
    /// The execution state that can be resumed.
    snapshot: EitherSnapshot,
    /// Name of the script being executed.
    script_name: String,
    /// The name of the external function being called.
    function_name: String,
    /// The positional arguments passed to the function (stored as MontyObject for serialization).
    args: Vec<MontyObject>,
    /// The keyword arguments passed to the function (stored as MontyObject pairs for serialization).
    kwargs: Vec<(MontyObject, MontyObject)>,
}

/// Options for resuming execution.
#[napi(object)]
pub struct ResumeOptions {
    /// The value to return from the external function call.
    pub return_value: Option<Value>,
    /// An exception to raise in the interpreter.
    /// Format: { type: string, message: string }
    pub exception: Option<ExceptionInput>,
}

/// Input for raising an exception during resume.
#[napi(object)]
pub struct ExceptionInput {
    /// The exception type name (e.g., "ValueError").
    pub r#type: String,
    /// The exception message.
    pub message: String,
}

/// Options for loading a serialized snapshot.
#[napi(object)]
pub struct SnapshotLoadOptions {
    // Future: could add dataclass-like registry support
}

#[napi]
impl MontySnapshot {
    /// Returns the name of the script being executed.
    #[napi(getter)]
    pub fn script_name(&self) -> String {
        self.script_name.clone()
    }

    /// Returns the name of the external function being called.
    #[napi(getter)]
    pub fn function_name(&self) -> String {
        self.function_name.clone()
    }

    /// Returns the positional arguments passed to the external function.
    #[napi(getter)]
    pub fn args<'env>(&self, env: &'env Env) -> Result<Vec<JsMontyObject<'env>>> {
        self.args.iter().map(|obj| monty_to_js(obj, env)).collect()
    }

    /// Returns the keyword arguments passed to the external function as an object.
    #[napi(getter)]
    pub fn kwargs<'env>(&self, env: &'env Env) -> Result<Object<'env>> {
        let mut obj = Object::new(env)?;
        for (k, v) in &self.kwargs {
            // Keys should be strings
            let key = match k {
                MontyObject::String(s) => s.clone(),
                _ => format!("{k:?}"),
            };
            let js_value = monty_to_js(v, env)?;
            obj.set_named_property(&key, js_value)?;
        }
        Ok(obj)
    }

    /// Resumes execution with either a return value or an exception.
    ///
    /// Exactly one of `returnValue` or `exception` must be provided.
    ///
    /// @param options - Object with either `returnValue` or `exception`
    /// @returns MontySnapshot if another external call is pending, MontyComplete if done
    #[napi]
    pub fn resume(&mut self, options: ResumeOptions) -> Result<Either<Self, MontyComplete>> {
        // Validate that exactly one of returnValue or exception is provided
        let external_result = match (options.return_value, options.exception) {
            (Some(value), None) => {
                let monty_value = serde_to_monty(&value)?;
                ExternalResult::Return(monty_value)
            }
            (None, Some(exc)) => {
                let exc_type = string_to_exc_type(&exc.r#type);
                let monty_exc = MontyException::new(exc_type, Some(exc.message));
                ExternalResult::Error(monty_exc)
            }
            (Some(_), Some(_)) => {
                return Err(Error::from_reason(
                    "resume() accepts either returnValue or exception, not both",
                ));
            }
            (None, None) => {
                return Err(Error::from_reason("resume() requires either returnValue or exception"));
            }
        };

        // Take the snapshot, replacing with Done
        let snapshot = std::mem::replace(&mut self.snapshot, EitherSnapshot::Done);

        // Resume execution based on the snapshot type
        let mut print_output = CollectStringPrint::default();
        match snapshot {
            EitherSnapshot::NoLimit(state) => {
                let progress = state
                    .run(external_result, &mut print_output)
                    .map_err(|e| monty_exception_to_error(&e))?;
                Ok(progress_to_result(progress, self.script_name.clone()))
            }
            EitherSnapshot::Limited(state) => {
                let progress = state
                    .run(external_result, &mut print_output)
                    .map_err(|e| monty_exception_to_error(&e))?;
                Ok(progress_to_result(progress, self.script_name.clone()))
            }
            EitherSnapshot::Done => Err(Error::from_reason("Snapshot has already been resumed")),
        }
    }

    /// Serializes the MontySnapshot to a binary format.
    ///
    /// The serialized data can be stored and later restored with `MontySnapshot.load()`.
    /// This allows suspending execution and resuming later, potentially in a different process.
    ///
    /// @returns Buffer containing the serialized snapshot
    #[napi]
    pub fn dump(&self) -> Result<Buffer> {
        if matches!(self.snapshot, EitherSnapshot::Done) {
            return Err(Error::from_reason("Cannot dump snapshot that has already been resumed"));
        }

        let serialized = SerializedSnapshot {
            snapshot: &self.snapshot,
            script_name: &self.script_name,
            function_name: &self.function_name,
            args: &self.args,
            kwargs: &self.kwargs,
        };

        let bytes =
            postcard::to_allocvec(&serialized).map_err(|e| Error::from_reason(format!("Serialization failed: {e}")))?;
        Ok(Buffer::from(bytes))
    }

    /// Deserializes a MontySnapshot from binary format.
    ///
    /// @param data - The serialized snapshot data from `dump()`
    /// @param options - Optional load options (reserved for future use)
    /// @returns A new MontySnapshot instance
    #[napi(factory)]
    pub fn load(data: Buffer, _options: Option<SnapshotLoadOptions>) -> Result<Self> {
        let serialized: SerializedSnapshotOwned =
            postcard::from_bytes(&data).map_err(|e| Error::from_reason(format!("Deserialization failed: {e}")))?;

        Ok(Self {
            snapshot: serialized.snapshot,
            script_name: serialized.script_name,
            function_name: serialized.function_name,
            args: serialized.args,
            kwargs: serialized.kwargs,
        })
    }

    /// Returns a string representation of the MontySnapshot.
    #[napi]
    pub fn repr(&self) -> String {
        format!(
            "MontySnapshot(scriptName='{}', functionName='{}', args={:?}, kwargs={:?})",
            self.script_name, self.function_name, self.args, self.kwargs
        )
    }
}

// =============================================================================
// MontyComplete - Completed execution
// =============================================================================

/// Represents completed execution with a final output value.
#[napi]
pub struct MontyComplete {
    /// The final output value from the executed code (stored as JSON for lifetime management).
    output_value: Value,
}

#[napi]
impl MontyComplete {
    /// Returns the final output value from the executed code.
    #[napi(getter)]
    pub fn output<'env>(&self, env: &'env Env) -> Result<JsMontyObject<'env>> {
        let monty_obj = serde_to_monty(&self.output_value)?;
        monty_to_js(&monty_obj, env)
    }

    /// Returns a string representation of the MontyComplete.
    #[napi]
    #[must_use]
    pub fn repr(&self) -> String {
        format!("MontyComplete(output={:?})", self.output_value)
    }
}

// =============================================================================
// Helper functions for progress conversion
// =============================================================================

/// Converts a `RunProgress` to either a `MontySnapshot` or `MontyComplete`.
fn progress_to_result<T>(progress: RunProgress<T>, script_name: String) -> Either<MontySnapshot, MontyComplete>
where
    T: ResourceTracker + serde::Serialize + serde::de::DeserializeOwned,
    EitherSnapshot: FromSnapshot<T>,
{
    match progress {
        RunProgress::Complete(result) => {
            let output_value = monty_to_serde(&result);
            Either::B(MontyComplete { output_value })
        }
        RunProgress::FunctionCall {
            function_name,
            args,
            kwargs,
            state,
        } => {
            // Store args/kwargs as MontyObject directly for serialization
            Either::A(MontySnapshot {
                snapshot: EitherSnapshot::from_snapshot(state),
                script_name,
                function_name,
                args,
                kwargs,
            })
        }
    }
}

/// Trait to convert a typed Snapshot into EitherSnapshot.
trait FromSnapshot<T: ResourceTracker> {
    fn from_snapshot(snapshot: Snapshot<T>) -> Self;
}

impl FromSnapshot<NoLimitTracker> for EitherSnapshot {
    fn from_snapshot(snapshot: Snapshot<NoLimitTracker>) -> Self {
        Self::NoLimit(snapshot)
    }
}

impl FromSnapshot<LimitedTracker> for EitherSnapshot {
    fn from_snapshot(snapshot: Snapshot<LimitedTracker>) -> Self {
        Self::Limited(snapshot)
    }
}

/// Converts a string exception type to `ExcType`.
fn string_to_exc_type(type_name: &str) -> ExcType {
    match type_name {
        "Exception" => ExcType::Exception,
        "BaseException" => ExcType::BaseException,
        "SystemExit" => ExcType::SystemExit,
        "KeyboardInterrupt" => ExcType::KeyboardInterrupt,
        "ArithmeticError" => ExcType::ArithmeticError,
        "OverflowError" => ExcType::OverflowError,
        "ZeroDivisionError" => ExcType::ZeroDivisionError,
        "LookupError" => ExcType::LookupError,
        "IndexError" => ExcType::IndexError,
        "KeyError" => ExcType::KeyError,
        "RuntimeError" => ExcType::RuntimeError,
        "NotImplementedError" => ExcType::NotImplementedError,
        "RecursionError" => ExcType::RecursionError,
        "AssertionError" => ExcType::AssertionError,
        "AttributeError" => ExcType::AttributeError,
        "MemoryError" => ExcType::MemoryError,
        "NameError" => ExcType::NameError,
        "UnboundLocalError" => ExcType::UnboundLocalError,
        "SyntaxError" => ExcType::SyntaxError,
        "TimeoutError" => ExcType::TimeoutError,
        "TypeError" => ExcType::TypeError,
        "ValueError" => ExcType::ValueError,
        "ImportError" => ExcType::ImportError,
        "ModuleNotFoundError" => ExcType::ModuleNotFoundError,
        "UnicodeDecodeError" => ExcType::UnicodeDecodeError,
        _ => ExcType::Exception, // Default to generic Exception
    }
}

// =============================================================================
// Serialization types
// =============================================================================

/// Serialization wrapper for `Monty` that includes all fields needed for reconstruction.
#[derive(serde::Serialize, serde::Deserialize)]
struct SerializedMonty {
    runner: MontyRun,
    script_name: String,
    input_names: Vec<String>,
    external_function_names: Vec<String>,
}

/// Serialization wrapper for `MontySnapshot` using borrowed references.
#[derive(serde::Serialize)]
struct SerializedSnapshot<'a> {
    snapshot: &'a EitherSnapshot,
    script_name: &'a str,
    function_name: &'a str,
    args: &'a [MontyObject],
    kwargs: &'a [(MontyObject, MontyObject)],
}

/// Owned version of `SerializedSnapshot` for deserialization.
#[derive(serde::Deserialize)]
struct SerializedSnapshotOwned {
    snapshot: EitherSnapshot,
    script_name: String,
    function_name: String,
    args: Vec<MontyObject>,
    kwargs: Vec<(MontyObject, MontyObject)>,
}
