//! The main `Monty` class and iterative execution support for the TypeScript/JavaScript bindings.
//!
//! Provides a sandboxed Python interpreter that can be configured with inputs
//! and resource limits. External functions are provided at runtime via
//! `RunOptions` or `StartOptions`. Supports both immediate execution
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
//! Monty.start() -> MontySnapshot | MontyNameLookup | MontyComplete
//!                       |                |
//!                       v                v
//! MontySnapshot.resume() / MontyNameLookup.resume()
//!       -> MontySnapshot | MontyNameLookup | MontyComplete
//!                       |                |
//!                       v                v
//!                    (repeat until complete)
//! ```
//!
//! ```typescript
//! const m = new Monty('result = external_func(1, 2)');
//!
//! let progress = m.start();
//! while (progress instanceof MontySnapshot) {
//!   console.log(`Calling ${progress.functionName} with args:`, progress.args);
//!   progress = progress.resume({ returnValue: 42 });
//! }
//! console.log('Final result:', progress.output);
//! ```

use std::{borrow::Cow, fmt::Write, mem, ptr, result};

use monty::{
    fs::MountTable, ExcType, ExtFunctionResult, FunctionCall, LimitedTracker, MontyException, MontyObject,
    MontyRepl as CoreMontyRepl, MontyRun, NameLookup, NameLookupResult, NoLimitTracker, OsCall, PrintWriter,
    PrintWriterCallback, ReplProgress, ResourceTracker, RunProgress,
};
use monty_type_checking::{type_check, SourceFile};
use napi::{bindgen_prelude::*, sys::Status};
use napi_derive::napi;
use serde::de::DeserializeOwned;

use crate::{
    convert::{js_to_monty, monty_to_js, JsMontyObject},
    exceptions::{exc_js_to_monty, JsMontyException, MontyTypingError},
    limits::JsResourceLimits,
    mount::{extract_mounts, OsHandler},
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
}

/// Options for creating a new Monty instance.
#[napi(object)]
#[derive(Default)]
pub struct MontyOptions {
    /// Name used in tracebacks and error messages. Default: 'main.py'
    pub script_name: Option<String>,
    /// List of input variable names available in the code.
    pub inputs: Option<Vec<String>>,
    /// Whether to perform type checking on the code. Default: false
    pub type_check: Option<bool>,
    /// Optional code to prepend before type checking.
    pub type_check_prefix_code: Option<String>,
}

/// Options for running code.
#[napi(object)]
#[derive(Default)]
pub struct RunOptions<'env> {
    pub inputs: Option<Object<'env>>,
    /// Resource limits configuration.
    pub limits: Option<JsResourceLimits>,
    /// Optional print callback function.
    pub print_callback: Option<JsPrintCallback<'env>>,
    /// Dict of external function callbacks.
    /// Keys are function names, values are callable functions.
    pub external_functions: Option<Object<'env>>,
    /// Filesystem mount(s) for the sandbox.
    /// A single `MountDir` or an array of `MountDir`.
    pub mount: Option<Object<'env>>,
}

/// Options for starting execution.
#[napi(object)]
#[derive(Default)]
pub struct StartOptions<'env> {
    /// Dict of input variable values.
    pub inputs: Option<Object<'env>>,
    /// Resource limits configuration.
    pub limits: Option<JsResourceLimits>,
    /// Optional print callback function.
    pub print_callback: Option<JsPrintCallback<'env>>,
    /// Filesystem mount(s) for the sandbox.
    /// A single `MountDir` or an array of `MountDir`.
    pub mount: Option<Object<'env>>,
}

#[napi]
impl Monty {
    /// Creates a new Monty interpreter by parsing the given code.
    ///
    /// Returns either a Monty instance, a MontyException (for syntax errors), or a MontyTypingError.
    /// The wrapper should check the result type and throw the appropriate error.
    ///
    /// @param code - Python code to execute
    /// @param options - Configuration options
    /// @returns Monty instance on success, or error object on failure
    #[napi]
    pub fn create(
        code: String,
        options: Option<MontyOptions>,
    ) -> Result<Either3<Self, JsMontyException, MontyTypingError>> {
        let ResolvedMontyOptions {
            script_name,
            input_names,
            do_type_check,
            type_check_prefix_code,
        } = resolve_monty_options(options);

        // Perform type checking if requested
        if do_type_check {
            if let Some(error) = run_type_check_result(&code, &script_name, type_check_prefix_code.as_deref())? {
                return Ok(Either3::C(error));
            }
        }

        // Create the runner (parses the code)
        let runner = match MontyRun::new(code, &script_name, input_names.clone()) {
            Ok(r) => r,
            Err(exc) => return Ok(Either3::B(JsMontyException::new(exc))),
        };

        Ok(Either3::A(Self {
            runner,
            script_name,
            input_names,
        }))
    }

    /// Performs static type checking on the code.
    ///
    /// Returns either nothing (success) or a MontyTypingError.
    ///
    /// @param prefixCode - Optional code to prepend before type checking
    /// @returns null on success, or MontyTypingError on failure
    #[napi]
    pub fn type_check(&self, prefix_code: Option<String>) -> Result<Option<MontyTypingError>> {
        run_type_check_result(self.runner.code(), &self.script_name, prefix_code.as_deref())
    }

    /// Executes the code and returns the result, or an exception object if execution fails.
    ///
    /// If runtime `externalFunctions` are provided, the start/resume loop is used
    /// to dispatch external function calls and name lookups. Otherwise, code is
    /// executed directly.
    ///
    /// @param options - Execution options (inputs, limits, externalFunctions)
    /// @returns The result of the last expression, or a MontyException if execution fails
    #[napi]
    pub fn run<'env>(
        &self,
        env: &'env Env,
        options: Option<RunOptions<'env>>,
    ) -> Result<Either<JsMontyObject<'env>, JsMontyException>> {
        let options = options.unwrap_or_default();
        let input_values = self.extract_input_values(options.inputs, *env)?;

        let external_functions = options.external_functions;
        let os_handler = match options.mount.as_ref() {
            Some(obj) => OsHandler::from_extracted(extract_mounts(obj)?),
            None => None,
        };

        let mut print_cb;
        let print_writer = match &options.print_callback {
            Some(func) => {
                print_cb = CallbackStringPrint::new_js(env, func)?;
                PrintWriter::Callback(&mut print_cb)
            }
            None => PrintWriter::Stdout,
        };

        // If we have runtime external functions or mounts, use the start/resume
        // loop to handle FunctionCall, NameLookup, and OsCall dispatching
        if external_functions.is_some() || os_handler.is_some() {
            return self.run_with_dispatch_loop(
                env,
                input_values,
                options.limits,
                external_functions,
                os_handler,
                print_writer,
            );
        }

        let result = if let Some(limits) = options.limits {
            let tracker = LimitedTracker::new(limits.try_into()?);
            self.runner.run(input_values, tracker, print_writer)
        } else {
            let tracker = NoLimitTracker;
            self.runner.run(input_values, tracker, print_writer)
        };

        match result {
            Ok(value) => Ok(Either::A(monty_to_js(&value, env)?)),
            Err(exc) => Ok(Either::B(JsMontyException::new(exc))),
        }
    }

    /// Internal helper to run code with external function callbacks and/or mounts.
    ///
    /// Handles `FunctionCall`, `NameLookup`, and `OsCall` dispatch in a loop.
    /// For `NameLookup`, checks the runtime external functions map: if the name
    /// is found, resolves it as a `Function`; otherwise returns `Undefined`.
    /// For `OsCall`, dispatches to the mount table if available, otherwise
    /// returns `NotImplementedError`.
    fn run_with_dispatch_loop<'env>(
        &self,
        env: &'env Env,
        input_values: Vec<MontyObject>,
        limits: Option<JsResourceLimits>,
        external_functions: Option<Object<'env>>,
        os_handler: Option<OsHandler>,
        mut print_output: PrintWriter<'_>,
    ) -> Result<Either<JsMontyObject<'env>, JsMontyException>> {
        let runner = self.runner.clone();

        // Take mounts out of their shared slots for zero-overhead execution.
        let mut mount_table: Option<MountTable> = os_handler.as_ref().map(OsHandler::take).transpose()?;

        // Helper: put mounts back into shared slots.
        let put_back = |table: Option<MountTable>| {
            if let (Some(h), Some(table)) = (&os_handler, table) {
                h.put_back(table);
            }
        };

        // Helper macro to handle the execution loop for both tracker types
        macro_rules! run_loop {
            ($tracker:expr) => {{
                let progress = runner.start(input_values, $tracker, print_output.reborrow());

                let mut progress = match progress {
                    Ok(p) => p,
                    Err(exc) => {
                        put_back(mount_table);
                        return Ok(Either::B(JsMontyException::new(exc)));
                    }
                };

                loop {
                    match progress {
                        RunProgress::Complete(result) => {
                            put_back(mount_table);
                            return Ok(Either::A(monty_to_js(&result, env)?));
                        }
                        RunProgress::FunctionCall(call) => {
                            let return_value = call_external_function(
                                env,
                                external_functions.as_ref(),
                                &call.function_name,
                                &call.args,
                                &call.kwargs,
                            )?;

                            progress = match call.resume(return_value, print_output.reborrow()) {
                                Ok(p) => p,
                                Err(exc) => {
                                    put_back(mount_table);
                                    return Ok(Either::B(JsMontyException::new(exc)));
                                }
                            };
                        }
                        RunProgress::NameLookup(lookup) => {
                            let result = resolve_name_lookup(external_functions.as_ref(), &lookup.name)?;
                            progress = match lookup.resume(result, print_output.reborrow()) {
                                Ok(p) => p,
                                Err(exc) => {
                                    put_back(mount_table);
                                    return Ok(Either::B(JsMontyException::new(exc)));
                                }
                            };
                        }
                        RunProgress::ResolveFutures(_) => {
                            put_back(mount_table);
                            return Err(Error::from_reason(
                                "Async futures are not supported in synchronous run(). Use start() for async execution.",
                            ));
                        }
                        RunProgress::OsCall(call) => {
                            let result = handle_os_call(&call, &mut mount_table);
                            progress = match call.resume(result, print_output.reborrow()) {
                                Ok(p) => p,
                                Err(exc) => {
                                    put_back(mount_table);
                                    return Ok(Either::B(JsMontyException::new(exc)));
                                }
                            };
                        }
                    }
                }
            }};
        }

        if let Some(resource_limits) = limits {
            let tracker = match resource_limits.try_into() {
                Ok(r) => LimitedTracker::new(r),
                Err(err) => {
                    put_back(mount_table);
                    return Err(err);
                }
            };
            run_loop!(tracker)
        } else {
            run_loop!(NoLimitTracker)
        }
    }

    /// Starts execution and returns a snapshot (paused at external call or name lookup),
    /// completion, or error.
    ///
    /// This method enables iterative execution where code pauses at external function
    /// calls or name lookups, allowing the host to provide return values before resuming.
    ///
    /// @param options - Execution options (inputs, limits)
    /// @returns MontySnapshot if paused at function call, MontyNameLookup if paused at
    ///   name lookup, MontyComplete if done, or MontyException if failed
    #[napi]
    pub fn start<'env>(
        &self,
        env: &'env Env,
        options: Option<StartOptions<'env>>,
    ) -> Result<Either4<MontySnapshot, MontyNameLookup, MontyComplete, JsMontyException>> {
        let options = options.unwrap_or_default();
        let input_values = self.extract_input_values(options.inputs, *env)?;

        // Build the mount handler from the mount parameter
        let os_handler = match options.mount.as_ref() {
            Some(obj) => OsHandler::from_extracted(extract_mounts(obj)?),
            None => None,
        };
        let mount_table: Option<MountTable> = os_handler.as_ref().map(OsHandler::take).transpose()?;
        let mount_state = os_handler.zip(mount_table);

        // Clone the runner since start() consumes it - allows reuse of the parsed code
        let runner = self.runner.clone();

        // Build print writer and capture the callback ref for the snapshot
        let mut print_cb;
        let print_writer = match &options.print_callback {
            Some(func) => {
                print_cb = CallbackStringPrint::new_js(env, func)?;
                PrintWriter::Callback(&mut print_cb)
            }
            None => PrintWriter::Stdout,
        };
        let print_callback_ref = options.print_callback.as_ref().map(Function::create_ref).transpose()?;

        // Start execution with appropriate tracker
        if let Some(resource_limits) = options.limits {
            let tracker = match resource_limits.try_into() {
                Ok(r) => LimitedTracker::new(r),
                Err(err) => {
                    put_back_mount_state(mount_state);
                    return Err(err);
                }
            };
            let progress = match runner.start(input_values, tracker, print_writer) {
                Ok(p) => p,
                Err(exc) => {
                    put_back_mount_state(mount_state);
                    return Ok(Either4::D(JsMontyException::new(exc)));
                }
            };
            progress_to_result_with_mounts(env, progress, print_callback_ref, self.script_name(), mount_state)
        } else {
            let tracker = NoLimitTracker;
            let progress = match runner.start(input_values, tracker, print_writer) {
                Ok(p) => p,
                Err(exc) => {
                    put_back_mount_state(mount_state);
                    return Ok(Either4::D(JsMontyException::new(exc)));
                }
            };
            progress_to_result_with_mounts(env, progress, print_callback_ref, self.script_name(), mount_state)
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

    /// Returns a string representation of the Monty instance.
    #[napi]
    pub fn repr(&self) -> String {
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
        s.push(')');
        s
    }

    /// Extracts input values from the JS Object in the order they were declared.
    fn extract_input_values(&self, inputs: Option<Object<'_>>, env: Env) -> Result<Vec<MontyObject>> {
        extract_input_values_in_order(&self.input_names, inputs, env)
    }
}

/// Performs type checking on the code and returns the error object if there are type errors.
///
/// Returns `None` if type checking passes, or `Some(MontyTypingError)` if there are errors.
fn run_type_check_result(code: &str, script_name: &str, prefix_code: Option<&str>) -> Result<Option<MontyTypingError>> {
    let source_code: Cow<str> = if let Some(prefix_code) = prefix_code {
        format!("{prefix_code}\n{code}").into()
    } else {
        code.into()
    };

    let source_file = SourceFile::new(&source_code, script_name);
    let result =
        type_check(&source_file, None).map_err(|e| Error::from_reason(format!("Type checking failed: {e}")))?;

    Ok(result.map(MontyTypingError::from_failure))
}

// =============================================================================
// MontyRepl - Incremental no-replay REPL session
// =============================================================================

/// REPL state holder for napi interoperability.
///
/// `napi` classes cannot be generic, so this enum stores REPL sessions for both
/// resource tracker variants.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum EitherRepl {
    NoLimit(CoreMontyRepl<NoLimitTracker>),
    Limited(CoreMontyRepl<LimitedTracker>),
}

/// Options for creating a new `MontyRepl` instance.
///
/// Controls the script name shown in tracebacks and optional resource limits
/// that apply to all subsequent `feed()` calls.
#[napi(object)]
#[derive(Default)]
pub struct MontyReplOptions {
    /// Name used in tracebacks and error messages. Default: 'main.py'
    pub script_name: Option<String>,
    /// Resource limits configuration applied to all snippet executions.
    pub limits: Option<JsResourceLimits>,
}

/// Options for `MontyRepl.feed()`.
#[napi(object)]
#[derive(Default)]
pub struct FeedOptions<'env> {
    /// Filesystem mount(s) for the sandbox.
    /// A single `MountDir` or an array of `MountDir`.
    pub mount: Option<Object<'env>>,
}

/// Stateful no-replay REPL session.
///
/// Create with `new MontyRepl()` then call `feed()` to execute snippets
/// incrementally against persistent heap and namespace state.
#[napi]
pub struct MontyRepl {
    repl: Option<EitherRepl>,
    script_name: String,
}

#[napi]
impl MontyRepl {
    /// Creates an empty REPL session ready to receive snippets via `feed()`.
    ///
    /// No code is parsed or executed at construction time — all execution
    /// is driven through `feed()`.
    ///
    /// @param options - Optional configuration (scriptName, limits)
    #[napi(constructor)]
    pub fn new(options: Option<MontyReplOptions>) -> Result<Self> {
        let options = options.unwrap_or_default();
        let script_name = options.script_name.unwrap_or_else(|| "main.py".to_string());

        let repl = if let Some(limits) = options.limits {
            let tracker = LimitedTracker::new(limits.try_into()?);
            EitherRepl::Limited(CoreMontyRepl::new(&script_name, tracker))
        } else {
            EitherRepl::NoLimit(CoreMontyRepl::new(&script_name, NoLimitTracker))
        };

        Ok(Self {
            repl: Some(repl),
            script_name,
        })
    }

    /// Returns the script name for this REPL session.
    #[napi(getter)]
    #[must_use]
    pub fn script_name(&self) -> String {
        self.script_name.clone()
    }

    /// Executes one incremental snippet against persistent REPL state.
    ///
    /// @param code - Python code to execute
    /// @param options - Optional feed options (mount)
    #[napi]
    pub fn feed<'env>(
        &mut self,
        env: &'env Env,
        code: String,
        options: Option<FeedOptions<'env>>,
    ) -> Result<Either<JsMontyObject<'env>, JsMontyException>> {
        let options = options.unwrap_or_default();
        let os_handler = match options.mount.as_ref() {
            Some(obj) => OsHandler::from_extracted(extract_mounts(obj)?),
            None => None,
        };

        // If mounts are provided, use feed_start/resume loop to handle OsCall
        if os_handler.is_some() {
            return self.feed_with_mounts(env, code, os_handler);
        }

        let repl = self
            .repl
            .as_mut()
            .ok_or_else(|| Error::from_reason("REPL session is currently in use"))?;

        let output = match repl {
            EitherRepl::NoLimit(repl) => repl.feed_run(&code, vec![], PrintWriter::Stdout),
            EitherRepl::Limited(repl) => repl.feed_run(&code, vec![], PrintWriter::Stdout),
        };

        match output {
            Ok(value) => Ok(Either::A(monty_to_js(&value, env)?)),
            Err(exc) => Ok(Either::B(JsMontyException::new(exc))),
        }
    }

    /// Serializes this REPL session to bytes.
    #[napi]
    pub fn dump(&self) -> Result<Buffer> {
        let repl = self
            .repl
            .as_ref()
            .ok_or_else(|| Error::from_reason("REPL session is currently in use"))?;
        let serialized = SerializedRepl {
            repl,
            script_name: &self.script_name,
        };
        let bytes =
            postcard::to_allocvec(&serialized).map_err(|e| Error::from_reason(format!("Serialization failed: {e}")))?;
        Ok(Buffer::from(bytes))
    }

    /// Restores a REPL session from bytes produced by `dump()`.
    #[napi(factory)]
    pub fn load(data: Buffer) -> Result<Self> {
        let serialized: SerializedReplOwned =
            postcard::from_bytes(&data).map_err(|e| Error::from_reason(format!("Deserialization failed: {e}")))?;
        Ok(Self {
            repl: Some(serialized.repl),
            script_name: serialized.script_name,
        })
    }

    /// Returns a string representation of the REPL session.
    #[napi]
    #[must_use]
    pub fn repr(&self) -> String {
        format!("MontyRepl(scriptName='{}')", self.script_name)
    }
}

impl MontyRepl {
    /// Executes a REPL snippet with mount support using the feed_start/resume loop.
    ///
    /// Takes the REPL out of `self.repl` for the duration of the loop (since
    /// `feed_start` consumes the REPL) and restores it when complete.
    fn feed_with_mounts<'env>(
        &mut self,
        env: &'env Env,
        code: String,
        os_handler: Option<OsHandler>,
    ) -> Result<Either<JsMontyObject<'env>, JsMontyException>> {
        // Take mounts out of shared slots
        let mut mount_table: Option<MountTable> = os_handler.as_ref().map(OsHandler::take).transpose()?;

        let put_back = |table: Option<MountTable>| {
            if let (Some(h), Some(table)) = (&os_handler, table) {
                h.put_back(table);
            }
        };

        // Take the REPL out (feed_start consumes it)
        let repl = self
            .repl
            .take()
            .ok_or_else(|| Error::from_reason("REPL session is currently in use"))?;

        macro_rules! feed_loop {
            ($repl:expr) => {{
                let progress = match $repl.feed_start(&code, vec![], PrintWriter::Stdout) {
                    Ok(p) => p,
                    Err(e) => {
                        // Restore REPL from error
                        self.repl = Some(EitherRepl::from_core(e.repl));
                        put_back(mount_table);
                        return Ok(Either::B(JsMontyException::new(e.error)));
                    }
                };

                let mut progress = progress;
                loop {
                    match progress {
                        ReplProgress::Complete { repl, value } => {
                            self.repl = Some(EitherRepl::from_core(repl));
                            put_back(mount_table);
                            return Ok(Either::A(monty_to_js(&value, env)?));
                        }
                        ReplProgress::OsCall(call) => {
                            let os_result = if let Some(ref mut table) = mount_table {
                                handle_os_call_with_table_repl(&call, table)
                            } else {
                                call.function.on_no_handler(&call.args).into()
                            };
                            progress = match call.resume(os_result, PrintWriter::Stdout) {
                                Ok(p) => p,
                                Err(e) => {
                                    self.repl = Some(EitherRepl::from_core(e.repl));
                                    put_back(mount_table);
                                    return Ok(Either::B(JsMontyException::new(e.error)));
                                }
                            };
                        }
                        ReplProgress::FunctionCall(call) => {
                            // No external function support in feed — return error
                            let func_name = call.function_name.clone();
                            self.repl = Some(EitherRepl::from_core(call.into_repl()));
                            put_back(mount_table);
                            return Ok(Either::B(JsMontyException::new(MontyException::new(
                                ExcType::RuntimeError,
                                Some(format!(
                                    "External function '{func_name}' called but REPL feed() does not support external functions",
                                )),
                            ))));
                        }
                        ReplProgress::NameLookup(lookup) => {
                            // No name lookup support — let it raise NameError
                            progress = match lookup.resume(NameLookupResult::Undefined, PrintWriter::Stdout) {
                                Ok(p) => p,
                                Err(e) => {
                                    self.repl = Some(EitherRepl::from_core(e.repl));
                                    put_back(mount_table);
                                    return Ok(Either::B(JsMontyException::new(e.error)));
                                }
                            };
                        }
                        ReplProgress::ResolveFutures(state) => {
                            self.repl = Some(EitherRepl::from_core(state.into_repl()));
                            put_back(mount_table);
                            return Err(Error::from_reason(
                                "Async futures are not supported in REPL feed()",
                            ));
                        }
                    }
                }
            }};
        }

        match repl {
            EitherRepl::NoLimit(repl) => feed_loop!(repl),
            EitherRepl::Limited(repl) => feed_loop!(repl),
        }
    }
}

/// Handles an OS call from the REPL using a mount table.
fn handle_os_call_with_table_repl<T: ResourceTracker>(
    call: &monty::ReplOsCall<T>,
    table: &mut MountTable,
) -> ExtFunctionResult {
    match table.handle_os_call(call.function, &call.args, &call.kwargs) {
        Some(Ok(obj)) => obj.into(),
        Some(Err(mount_err)) => mount_err.into_exception().into(),
        None => call.function.on_no_handler(&call.args).into(),
    }
}

/// Trait to convert a typed `CoreMontyRepl<T>` back into `EitherRepl`.
trait FromCoreRepl<T: ResourceTracker> {
    /// Wraps a core REPL into the appropriate `EitherRepl` variant.
    fn from_core(repl: CoreMontyRepl<T>) -> Self;
}

impl FromCoreRepl<NoLimitTracker> for EitherRepl {
    fn from_core(repl: CoreMontyRepl<NoLimitTracker>) -> Self {
        Self::NoLimit(repl)
    }
}

impl FromCoreRepl<LimitedTracker> for EitherRepl {
    fn from_core(repl: CoreMontyRepl<LimitedTracker>) -> Self {
        Self::Limited(repl)
    }
}

/// Fully resolved creation options shared by `Monty` and `MontyRepl`.
///
/// This keeps parsing/type-checking defaults consistent across non-REPL and
/// REPL entry points.
struct ResolvedMontyOptions {
    script_name: String,
    input_names: Vec<String>,
    do_type_check: bool,
    type_check_prefix_code: Option<String>,
}

/// Normalizes optional JS-facing creation options into concrete defaults.
fn resolve_monty_options(options: Option<MontyOptions>) -> ResolvedMontyOptions {
    let options = options.unwrap_or(MontyOptions {
        script_name: None,
        inputs: None,
        type_check: None,
        type_check_prefix_code: None,
    });

    ResolvedMontyOptions {
        script_name: options.script_name.unwrap_or_else(|| "main.py".to_string()),
        input_names: options.inputs.unwrap_or_default(),
        do_type_check: options.type_check.unwrap_or(false),
        type_check_prefix_code: options.type_check_prefix_code,
    }
}

/// Extracts input values in declaration order from a JS object.
///
/// This helper is shared by regular `Monty` execution and direct REPL creation
/// so both paths perform identical input validation.
fn extract_input_values_in_order(
    input_names: &[String],
    inputs: Option<Object<'_>>,
    env: Env,
) -> Result<Vec<MontyObject>> {
    if input_names.is_empty() {
        if inputs.is_some() {
            return Err(Error::from_reason(
                "No input variables declared but inputs object was provided",
            ));
        }
        return Ok(vec![]);
    }

    let Some(inputs) = inputs else {
        return Err(Error::from_reason(format!("Missing required inputs: {input_names:?}")));
    };

    input_names
        .iter()
        .map(|name| {
            if !inputs.has_named_property(name)? {
                return Err(Error::from_reason(format!("Missing required input: '{name}'")));
            }
            let value: Unknown = inputs.get_named_property(name)?;
            js_to_monty(value, env)
        })
        .collect()
}

// =============================================================================
// EitherSnapshot - Internal enum to handle generic resource tracker types
// =============================================================================

/// Runtime execution snapshot, holds a `FunctionCall` for either resource tracker variant
/// since napi structs can't be generic.
///
/// Used internally by `MontySnapshot` to store execution state.
/// The `Done` variant indicates the snapshot has been consumed.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum EitherSnapshot {
    NoLimit(FunctionCall<NoLimitTracker>),
    Limited(FunctionCall<LimitedTracker>),
    /// Sentinel indicating the snapshot has been consumed via `resume()`.
    Done,
}

// =============================================================================
// MontySnapshot - Paused execution at an external function call
// =============================================================================

/// Represents paused execution waiting for an external function call return value.
///
/// Contains information about the pending external function call and allows
/// resuming execution with the return value or an exception.
/// Active mount state carried across `start()`/`resume()` calls.
///
/// Contains the handler (with shared mount references for put-back) and the
/// taken mount table for zero-overhead filesystem operations.
type MountState = (OsHandler, MountTable);

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
    /// Optional print callback function.
    print_callback: Option<JsPrintCallbackRef>,
    /// Mount state carried from `start()` for use during `resume()`.
    mount_state: Option<MountState>,
}

/// Options for resuming execution.
#[napi(object)]
pub struct ResumeOptions<'env> {
    /// The value to return from the external function call.
    pub return_value: Option<Unknown<'env>>,
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
pub struct SnapshotLoadOptions<'env> {
    /// Optional print callback function.
    pub print_callback: Option<JsPrintCallback<'env>>,
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
    /// @returns MontySnapshot if paused at function call, MontyNameLookup if paused at
    ///   name lookup, MontyComplete if done, or MontyException if failed
    #[napi]
    pub fn resume<'env>(
        &mut self,
        env: &'env Env,
        options: ResumeOptions<'env>,
    ) -> Result<Either4<Self, MontyNameLookup, MontyComplete, JsMontyException>> {
        // Validate that exactly one of returnValue or exception is provided
        let external_result = match (options.return_value, options.exception) {
            (Some(value), None) => {
                let monty_value = js_to_monty(value, *env)?;
                ExtFunctionResult::Return(monty_value)
            }
            (None, Some(exc)) => {
                let monty_exc = MontyException::new(string_to_exc_type(&exc.r#type)?, Some(exc.message));
                ExtFunctionResult::Error(monty_exc)
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
        let snapshot = mem::replace(&mut self.snapshot, EitherSnapshot::Done);

        // Take the print callback
        // This is necessary to move out of `&mut self` to please the borrow checker.
        // Unless the entire snapshot generator is refactored we have to do this.
        let print_callback = mem::take(&mut self.print_callback);

        // Build print writer from the callback ref
        let mut print_cb;
        let print_writer = match &print_callback {
            Some(func) => {
                print_cb = CallbackStringPrint::new_js_ref(env, func)?;
                PrintWriter::Callback(&mut print_cb)
            }
            None => PrintWriter::Stdout,
        };

        // Take mount state from the snapshot
        let mount_state = mem::take(&mut self.mount_state);

        // Resume execution based on the snapshot type
        match snapshot {
            EitherSnapshot::NoLimit(call) => {
                let progress = match call.resume(external_result, print_writer) {
                    Ok(p) => p,
                    Err(exc) => {
                        put_back_mount_state(mount_state);
                        return Ok(Either4::D(JsMontyException::new(exc)));
                    }
                };
                progress_to_result_with_mounts(env, progress, print_callback, self.script_name.clone(), mount_state)
            }
            EitherSnapshot::Limited(call) => {
                let progress = match call.resume(external_result, print_writer) {
                    Ok(p) => p,
                    Err(exc) => {
                        put_back_mount_state(mount_state);
                        return Ok(Either4::D(JsMontyException::new(exc)));
                    }
                };
                progress_to_result_with_mounts(env, progress, print_callback, self.script_name.clone(), mount_state)
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
    pub fn load(data: Buffer, options: Option<SnapshotLoadOptions>) -> Result<Self> {
        let serialized: SerializedSnapshotOwned =
            postcard::from_bytes(&data).map_err(|e| Error::from_reason(format!("Deserialization failed: {e}")))?;

        Ok(Self {
            snapshot: serialized.snapshot,
            script_name: serialized.script_name,
            function_name: serialized.function_name,
            args: serialized.args,
            kwargs: serialized.kwargs,
            print_callback: options
                .as_ref()
                .and_then(|t| t.print_callback.as_ref())
                .map(Function::create_ref)
                .transpose()?,
            mount_state: None,
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
///
/// The output value is stored as a `MontyObject` internally and converted to JS on access.
#[napi]
pub struct MontyComplete {
    /// The final output value from the executed code.
    output_value: MontyObject,
}

#[napi]
impl MontyComplete {
    /// Returns the final output value from the executed code.
    #[napi(getter)]
    pub fn output<'env>(&self, env: &'env Env) -> Result<JsMontyObject<'env>> {
        monty_to_js(&self.output_value, env)
    }

    /// Returns a string representation of the MontyComplete.
    #[napi]
    #[must_use]
    pub fn repr(&self) -> String {
        format!("MontyComplete(output={:?})", self.output_value)
    }
}

// =============================================================================
// EitherLookupSnapshot - Internal enum for NameLookup tracker variants
// =============================================================================

/// Runtime execution snapshot, holds a `NameLookup` for either resource tracker variant
/// since napi structs can't be generic.
///
/// The `Done` variant indicates the snapshot has been consumed.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
enum EitherLookupSnapshot {
    NoLimit(NameLookup<NoLimitTracker>),
    Limited(NameLookup<LimitedTracker>),
    /// Sentinel indicating the snapshot has been consumed via `resume()`.
    Done,
}

/// Trait to convert a typed `NameLookup` into `EitherLookupSnapshot`.
trait FromLookupSnapshot<T: ResourceTracker> {
    /// Wraps a name-lookup snapshot.
    fn from_lookup(lookup: NameLookup<T>) -> Self;
}

impl FromLookupSnapshot<NoLimitTracker> for EitherLookupSnapshot {
    fn from_lookup(lookup: NameLookup<NoLimitTracker>) -> Self {
        Self::NoLimit(lookup)
    }
}

impl FromLookupSnapshot<LimitedTracker> for EitherLookupSnapshot {
    fn from_lookup(lookup: NameLookup<LimitedTracker>) -> Self {
        Self::Limited(lookup)
    }
}

// =============================================================================
// MontyNameLookup - Paused execution at a name lookup
// =============================================================================

/// Represents paused execution waiting for a name to be resolved.
///
/// The host should check if the variable name corresponds to a known value
/// (e.g., an external function). Call `resume()` with the value to continue
/// execution, or call `resume()` with no value to raise `NameError`.
#[napi]
pub struct MontyNameLookup {
    /// The execution state that can be resumed.
    snapshot: EitherLookupSnapshot,
    /// Name of the script being executed.
    script_name: String,
    /// The name of the variable being looked up.
    variable_name: String,
    /// Optional print callback function.
    print_callback: Option<JsPrintCallbackRef>,
    /// Mount state carried from `start()` for use during `resume()`.
    mount_state: Option<MountState>,
}

/// Options for resuming execution from a name lookup.
///
/// If `value` is provided, the name resolves to that value and execution continues.
/// If `value` is omitted or undefined, the VM raises a `NameError`.
#[napi(object)]
pub struct NameLookupResumeOptions<'env> {
    /// The value to provide for the name.
    pub value: Option<Unknown<'env>>,
}

/// Options for loading a serialized name lookup snapshot.
#[napi(object)]
pub struct NameLookupLoadOptions<'env> {
    /// Optional print callback function.
    pub print_callback: Option<JsPrintCallback<'env>>,
}

#[napi]
impl MontyNameLookup {
    /// Returns the name of the script being executed.
    #[napi(getter)]
    pub fn script_name(&self) -> String {
        self.script_name.clone()
    }

    /// Returns the name of the variable being looked up.
    #[napi(getter)]
    pub fn variable_name(&self) -> String {
        self.variable_name.clone()
    }

    /// Resumes execution after resolving the name lookup.
    ///
    /// If `value` is provided, the name resolves to that value and execution continues.
    /// If `value` is omitted or undefined, the VM raises a `NameError`.
    ///
    /// @param options - Optional object with `value` to resolve the name to
    /// @returns MontySnapshot if paused at function call, MontyNameLookup if paused at
    ///   another name lookup, MontyComplete if done, or MontyException if failed
    #[napi]
    pub fn resume<'env>(
        &mut self,
        env: &'env Env,
        options: Option<NameLookupResumeOptions<'env>>,
    ) -> Result<Either4<MontySnapshot, Self, MontyComplete, JsMontyException>> {
        let lookup_result = match options.and_then(|opts| opts.value) {
            Some(value) => {
                let monty_value = js_to_monty(value, *env)?;
                NameLookupResult::Value(monty_value)
            }
            None => NameLookupResult::Undefined,
        };

        // Take the snapshot, replacing with Done
        let snapshot = mem::replace(&mut self.snapshot, EitherLookupSnapshot::Done);

        // Take the print callback
        let print_callback = mem::take(&mut self.print_callback);

        // Build print writer from the callback ref
        let mut print_cb;
        let print_writer = match &print_callback {
            Some(func) => {
                print_cb = CallbackStringPrint::new_js_ref(env, func)?;
                PrintWriter::Callback(&mut print_cb)
            }
            None => PrintWriter::Stdout,
        };

        // Take mount state from the snapshot
        let mount_state = mem::take(&mut self.mount_state);

        match snapshot {
            EitherLookupSnapshot::NoLimit(lookup) => {
                let progress = match lookup.resume(lookup_result, print_writer) {
                    Ok(p) => p,
                    Err(exc) => {
                        put_back_mount_state(mount_state);
                        return Ok(Either4::D(JsMontyException::new(exc)));
                    }
                };
                progress_to_result_with_mounts(env, progress, print_callback, self.script_name.clone(), mount_state)
            }
            EitherLookupSnapshot::Limited(lookup) => {
                let progress = match lookup.resume(lookup_result, print_writer) {
                    Ok(p) => p,
                    Err(exc) => {
                        put_back_mount_state(mount_state);
                        return Ok(Either4::D(JsMontyException::new(exc)));
                    }
                };
                progress_to_result_with_mounts(env, progress, print_callback, self.script_name.clone(), mount_state)
            }
            EitherLookupSnapshot::Done => Err(Error::from_reason("Name lookup has already been resumed")),
        }
    }

    /// Serializes the MontyNameLookup to a binary format.
    ///
    /// The serialized data can be stored and later restored with `MontyNameLookup.load()`.
    ///
    /// @returns Buffer containing the serialized name lookup snapshot
    #[napi]
    pub fn dump(&self) -> Result<Buffer> {
        if matches!(self.snapshot, EitherLookupSnapshot::Done) {
            return Err(Error::from_reason(
                "Cannot dump name lookup that has already been resumed",
            ));
        }

        let serialized = SerializedNameLookup {
            snapshot: &self.snapshot,
            script_name: &self.script_name,
            variable_name: &self.variable_name,
        };

        let bytes =
            postcard::to_allocvec(&serialized).map_err(|e| Error::from_reason(format!("Serialization failed: {e}")))?;
        Ok(Buffer::from(bytes))
    }

    /// Deserializes a MontyNameLookup from binary format.
    ///
    /// @param data - The serialized data from `dump()`
    /// @param options - Optional load options
    /// @returns A new MontyNameLookup instance
    #[napi(factory)]
    pub fn load(data: Buffer, options: Option<NameLookupLoadOptions>) -> Result<Self> {
        let serialized: SerializedNameLookupOwned =
            postcard::from_bytes(&data).map_err(|e| Error::from_reason(format!("Deserialization failed: {e}")))?;

        Ok(Self {
            snapshot: serialized.snapshot,
            script_name: serialized.script_name,
            variable_name: serialized.variable_name,
            print_callback: options
                .as_ref()
                .and_then(|t| t.print_callback.as_ref())
                .map(Function::create_ref)
                .transpose()?,
            mount_state: None,
        })
    }

    /// Returns a string representation of the MontyNameLookup.
    #[napi]
    pub fn repr(&self) -> String {
        format!(
            "MontyNameLookup(scriptName='{}', variableName='{}')",
            self.script_name, self.variable_name
        )
    }
}

// Function type for JS callback used in `CallbackStringPrint`.
type JsPrintCallback<'env> = Function<'env, FnArgs<(&'static str, String)>, ()>;
type JsPrintCallbackRef = FunctionRef<FnArgs<(&'static str, String)>, ()>;

/// A `PrintWriter` implementation that calls a javascript callback for each print output.
///
/// This structure internally holds a `napi::Function`.
pub struct CallbackStringPrint<'env>(JsPrintCallback<'env>);

impl<'env> CallbackStringPrint<'env> {
    /// Creates a new `CallbackStringPrint` from a `JsFunction`.
    pub fn new_js(env: &'env Env, func: &JsPrintCallback<'env>) -> napi::Result<Self> {
        Ok(Self(func.create_ref()?.borrow_back(env)?))
    }

    /// Creates a new printer from a function reference.
    ///
    /// This will re-borrow the function reference for use in printing.
    pub fn new_js_ref(env: &'env Env, func: &JsPrintCallbackRef) -> napi::Result<Self> {
        Ok(Self(func.borrow_back(env)?))
    }
}

impl PrintWriterCallback for CallbackStringPrint<'_> {
    fn stdout_write(&mut self, output: Cow<'_, str>) -> result::Result<(), MontyException> {
        self.0
            .call(("stdout", output.as_ref().to_owned()).into())
            .map_err(exc_js_to_monty)?;
        Ok(())
    }

    fn stdout_push(&mut self, end: char) -> result::Result<(), MontyException> {
        self.0
            .call(("stdout", end.to_string()).into())
            .map_err(exc_js_to_monty)?;
        Ok(())
    }
}

// =============================================================================
// Helper functions for progress conversion
// =============================================================================

/// Converts a `RunProgress` to a JS result, handling `OsCall` via the mount table.
///
/// Unlike [`progress_to_result`], this function loops on `OsCall` events,
/// dispatching them to the mount table before returning to JavaScript.
/// Mount state is carried forward into snapshots so it persists across
/// `resume()` calls.
fn progress_to_result_with_mounts<T>(
    env: &Env,
    mut progress: RunProgress<T>,
    print_callback: Option<JsPrintCallbackRef>,
    script_name: String,
    mut mount_state: Option<MountState>,
) -> Result<Either4<MontySnapshot, MontyNameLookup, MontyComplete, JsMontyException>>
where
    T: ResourceTracker + serde::Serialize + DeserializeOwned,
    EitherSnapshot: FromSnapshot<T>,
    EitherLookupSnapshot: FromLookupSnapshot<T>,
{
    // Build a reusable print callback so OsCall resumes use the JS callback
    // instead of falling back to stdout.
    let mut print_cb = match &print_callback {
        Some(func) => Some(CallbackStringPrint::new_js_ref(env, func)?),
        None => None,
    };

    // Loop to handle OsCall events via the mount table before yielding to JS.
    loop {
        match progress {
            RunProgress::Complete(result) => {
                put_back_mount_state(mount_state);
                return Ok(Either4::C(MontyComplete { output_value: result }));
            }
            RunProgress::FunctionCall(call) => {
                let function_name = call.function_name.clone();
                let args = call.args.clone();
                let kwargs = call.kwargs.clone();
                return Ok(Either4::A(MontySnapshot {
                    snapshot: EitherSnapshot::from_snapshot(call),
                    script_name,
                    function_name,
                    args,
                    kwargs,
                    print_callback,
                    mount_state,
                }));
            }
            RunProgress::NameLookup(lookup) => {
                let variable_name = lookup.name.clone();
                return Ok(Either4::B(MontyNameLookup {
                    snapshot: EitherLookupSnapshot::from_lookup(lookup),
                    script_name,
                    variable_name,
                    print_callback,
                    mount_state,
                }));
            }
            RunProgress::ResolveFutures(_) => {
                put_back_mount_state(mount_state);
                return Ok(Either4::D(JsMontyException::new(MontyException::new(
                    ExcType::NotImplementedError,
                    Some("Async futures (ResolveFutures) are not yet supported in the JS bindings".to_owned()),
                ))));
            }
            RunProgress::OsCall(call) => {
                let os_result = if let Some((_, ref mut table)) = mount_state {
                    handle_os_call_with_table(&call, table)
                } else {
                    // No mounts configured — use on_no_handler for appropriate error
                    call.function.on_no_handler(&call.args).into()
                };
                let print_writer = match &mut print_cb {
                    Some(cb) => PrintWriter::Callback(cb),
                    None => PrintWriter::Stdout,
                };
                progress = match call.resume(os_result, print_writer) {
                    Ok(p) => p,
                    Err(exc) => {
                        put_back_mount_state(mount_state);
                        return Ok(Either4::D(JsMontyException::new(exc)));
                    }
                };
            }
        }
    }
}

/// Puts mount state back into shared slots, if present.
fn put_back_mount_state(mount_state: Option<MountState>) {
    if let Some((handler, table)) = mount_state {
        handler.put_back(table);
    }
}

/// Handles an OS call by dispatching to the mount table.
///
/// Returns the result from the mount table if it handles the call,
/// or falls back to [`OsFunction::on_no_handler`] for unhandled calls.
fn handle_os_call<T: ResourceTracker>(call: &OsCall<T>, mount_table: &mut Option<MountTable>) -> ExtFunctionResult {
    if let Some(table) = mount_table {
        handle_os_call_with_table(call, table)
    } else {
        call.function.on_no_handler(&call.args).into()
    }
}

/// Handles an OS call using a specific mount table.
fn handle_os_call_with_table<T: ResourceTracker>(call: &OsCall<T>, table: &mut MountTable) -> ExtFunctionResult {
    match table.handle_os_call(call.function, &call.args, &call.kwargs) {
        Some(Ok(obj)) => obj.into(),
        Some(Err(mount_err)) => mount_err.into_exception().into(),
        None => call.function.on_no_handler(&call.args).into(),
    }
}

/// Trait to convert a typed `FunctionCall` into `EitherSnapshot`.
trait FromSnapshot<T: ResourceTracker> {
    /// Wraps a function-call snapshot.
    fn from_snapshot(call: FunctionCall<T>) -> Self;
}

impl FromSnapshot<NoLimitTracker> for EitherSnapshot {
    fn from_snapshot(call: FunctionCall<NoLimitTracker>) -> Self {
        Self::NoLimit(call)
    }
}

impl FromSnapshot<LimitedTracker> for EitherSnapshot {
    fn from_snapshot(call: FunctionCall<LimitedTracker>) -> Self {
        Self::Limited(call)
    }
}

/// Converts a string exception type to `ExcType`.
fn string_to_exc_type(type_name: &str) -> Result<ExcType> {
    type_name
        .parse()
        .map_err(|_| Error::from_reason(format!("Invalid exception type: '{type_name}'")))
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
}

/// Serialization wrapper for `MontyRepl` using borrowed references.
#[derive(serde::Serialize)]
struct SerializedRepl<'a> {
    repl: &'a EitherRepl,
    script_name: &'a str,
}

/// Owned version of `SerializedRepl` for deserialization.
#[derive(serde::Deserialize)]
struct SerializedReplOwned {
    repl: EitherRepl,
    script_name: String,
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

/// Serialization wrapper for `MontyNameLookup` using borrowed references.
#[derive(serde::Serialize)]
struct SerializedNameLookup<'a> {
    snapshot: &'a EitherLookupSnapshot,
    script_name: &'a str,
    variable_name: &'a str,
}

/// Owned version of `SerializedNameLookup` for deserialization.
#[derive(serde::Deserialize)]
struct SerializedNameLookupOwned {
    snapshot: EitherLookupSnapshot,
    script_name: String,
    variable_name: String,
}

// =============================================================================
// External function support
// =============================================================================

/// Calls a JavaScript external function and returns the result.
///
/// Converts args/kwargs from Monty format, calls the JS function,
/// and converts the result back to Monty format (or an exception).
fn call_external_function(
    env: &Env,
    external_functions: Option<&Object<'_>>,
    function_name: &str,
    args: &[MontyObject],
    kwargs: &[(MontyObject, MontyObject)],
) -> Result<ExtFunctionResult> {
    // Get the external functions dict, or error if not provided
    let functions = external_functions.ok_or_else(|| {
        Error::from_reason(format!(
            "External function '{function_name}' called but no externalFunctions provided"
        ))
    })?;

    // Look up the function by name
    if !functions.has_named_property(function_name)? {
        // Return a NameError exception — matches Python's behavior for undefined names
        let exc = MontyException::new(
            ExcType::NameError,
            Some(format!("name '{function_name}' is not defined")),
        );
        return Ok(ExtFunctionResult::Error(exc));
    }

    let callable: Unknown = functions.get_named_property(function_name)?;

    // Convert positional arguments to JS
    let mut js_args: Vec<sys::napi_value> = Vec::with_capacity(args.len() + 1);
    for arg in args {
        js_args.push(monty_to_js(arg, env)?.raw());
    }

    // If we have kwargs, add them as a final object argument
    if !kwargs.is_empty() {
        let mut kwargs_obj = Object::new(env)?;
        for (key, value) in kwargs {
            let key_str = match key {
                MontyObject::String(s) => s.clone(),
                _ => format!("{key:?}"),
            };
            kwargs_obj.set_named_property(&key_str, monty_to_js(value, env)?)?;
        }
        js_args.push(kwargs_obj.raw());
    }

    // Get undefined for the 'this' argument
    let mut undefined_raw = ptr::null_mut();
    // SAFETY: [DH] - all arguments are valid and result is valid on success
    unsafe {
        sys::napi_get_undefined(env.raw(), &raw mut undefined_raw);
    }

    // Call the function using raw napi
    let mut result_raw = ptr::null_mut();
    // SAFETY: [DH] - all arguments are valid and result is valid on success
    let status = unsafe {
        sys::napi_call_function(
            env.raw(),
            undefined_raw, // this = undefined
            callable.raw(),
            js_args.len(),
            js_args.as_ptr(),
            &raw mut result_raw,
        )
    };

    if status != Status::napi_ok {
        // An error occurred - get the pending exception
        let mut is_exception = false;
        // SAFETY: [DH] - all arguments are valid
        unsafe { sys::napi_is_exception_pending(env.raw(), &raw mut is_exception) };

        if is_exception {
            let mut exception_raw = ptr::null_mut();
            // SAFETY: [DH] - all arguments are valid and exception_raw is valid on success
            let status = unsafe { sys::napi_get_and_clear_last_exception(env.raw(), &raw mut exception_raw) };

            if status != Status::napi_ok {
                // Failed to get the exception - return a generic error
                let exc = MontyException::new(
                    ExcType::RuntimeError,
                    Some("External function call failed and exception could not be retrieved".to_string()),
                );
                return Ok(ExtFunctionResult::Error(exc));
            }
            let exception_obj = Object::from_raw(env.raw(), exception_raw);
            let exc = extract_js_exception(exception_obj);
            return Ok(ExtFunctionResult::Error(exc));
        }

        // Generic error
        let exc = MontyException::new(ExcType::RuntimeError, Some("External function call failed".to_string()));
        return Ok(ExtFunctionResult::Error(exc));
    }

    // Convert the result back to Monty format
    // SAFETY: [DH] - result_raw is valid on success
    let result = unsafe { Unknown::from_raw_unchecked(env.raw(), result_raw) };
    let monty_result = js_to_monty(result, *env)?;
    Ok(ExtFunctionResult::Return(monty_result))
}

/// Extracts exception info from a JS exception object.
fn extract_js_exception(exception_obj: Object<'_>) -> MontyException {
    // Try to get the 'name' property (e.g., "ValueError")
    let name: result::Result<String, _> = exception_obj.get_named_property("name");
    // Try to get the 'message' property
    let message: result::Result<String, _> = exception_obj.get_named_property("message");

    let exc_type = name
        .ok()
        .and_then(|n| string_to_exc_type(&n).ok())
        .unwrap_or(ExcType::RuntimeError);
    let msg = message.ok();

    MontyException::new(exc_type, msg)
}

/// Resolves a name lookup against the runtime external functions map.
///
/// If the name exists as a property on the external functions object, returns
/// `NameLookupResult::Value` with a `Function` object. Otherwise returns
/// `NameLookupResult::Undefined` so the VM raises `NameError`.
fn resolve_name_lookup(external_functions: Option<&Object<'_>>, name: &str) -> Result<NameLookupResult> {
    if let Some(functions) = external_functions {
        if functions.has_named_property(name)? {
            return Ok(NameLookupResult::Value(MontyObject::Function {
                name: name.to_string(),
                docstring: None, // TODO, can we do better?
            }));
        }
    }
    Ok(NameLookupResult::Undefined)
}
