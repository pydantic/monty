//! The main `Monty` class for the TypeScript/JavaScript bindings.
//!
//! Provides a sandboxed Python interpreter that can be configured with inputs,
//! external functions, and resource limits.

use std::borrow::Cow;

use monty::{CollectStringPrint, LimitedTracker, MontyObject, MontyRun, NoLimitTracker};
use monty_type_checking::type_check;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde_json::Value;

use crate::{
    convert::{monty_to_serde, serde_to_monty},
    exceptions::{monty_exception_to_error, typing_failure_to_error},
    limits::JsResourceLimits,
};

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
    pub fn run(&self, options: Option<RunOptions>) -> Result<Value> {
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
            Ok(value) => Ok(monty_to_serde(&value)),
            Err(exc) => Err(monty_exception_to_error(&exc)),
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
}

impl Monty {
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

/// Serialization wrapper for `Monty` that includes all fields needed for reconstruction.
#[derive(serde::Serialize, serde::Deserialize)]
struct SerializedMonty {
    runner: MontyRun,
    script_name: String,
    input_names: Vec<String>,
    external_function_names: Vec<String>,
}
