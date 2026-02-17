//! Implementation of the `os` module.
//!
//! Provides a minimal implementation of Python's `os` module with:
//! - `getenv(key, default=None)`: Get a single environment variable
//! - `environ`: Property that returns the entire environment as a dict
//!
//! Other os functions are not implemented. OS operations require host involvement
//! via the `OsFunction` callback mechanism - Monty yields control to the host
//! which executes the operation and returns the result.

use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult},
    heap::{Heap, HeapData, HeapId},
    intern::{Interns, StaticStrings},
    modules::ModuleFunctions,
    os::OsFunction,
    resource::{ResourceError, ResourceTracker},
    types::{AttrCallResult, Module, Property, PyTrait},
    value::Value,
};

/// OS module functions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum OsFunctions {
    Getenv,
}

/// Creates the `os` module and allocates it on the heap.
///
/// The module provides:
/// - `getenv(key, default=None)`: Get a single environment variable
/// - `environ`: Property that returns the entire environment as a dict
///
/// Both operations yield to the host via `OsFunction` callbacks.
///
/// # Returns
/// A HeapId pointing to the newly allocated module.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Os);

    // os.getenv - function to get a single environment variable
    module.set_attr(
        StaticStrings::Getenv,
        Value::ModuleFunction(ModuleFunctions::Os(OsFunctions::Getenv)),
        heap,
        interns,
    );

    // os.environ - property that returns the entire environment as a dict
    module.set_attr(
        StaticStrings::Environ,
        Value::Property(Property::Os(OsFunction::GetEnviron)),
        heap,
        interns,
    );

    heap.allocate(HeapData::Module(module))
}

/// Dispatches a call to an os module function.
///
/// Returns `AttrCallResult::OsCall` for functions that need host involvement,
/// or `AttrCallResult::Value` for functions that can be computed immediately.
pub(super) fn call(
    heap: &mut Heap<impl ResourceTracker>,
    functions: OsFunctions,
    args: ArgValues,
) -> RunResult<AttrCallResult> {
    match functions {
        OsFunctions::Getenv => getenv(heap, args),
    }
}

/// Implementation of `os.getenv(key, default=None)`.
///
/// Returns the value of the environment variable `key` if it exists, or `default` if it doesn't.
/// This function yields to the host to perform the actual environment lookup.
///
/// # Arguments
/// * `heap` - The heap for any allocations
/// * `args` - Function arguments: `key` (required string), `default` (optional, defaults to None)
///
/// # Returns
/// `AttrCallResult::OsCall` with `OsFunction::Getenv` - the host should look up the
/// environment variable and return the value, or the default if not found.
///
/// # Errors
/// Returns `TypeError` if:
/// - No arguments are provided
/// - More than 2 arguments are provided
/// - `key` is not a string
fn getenv(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<AttrCallResult> {
    // getenv(key, default=None) - accepts 1 or 2 positional arguments
    let (key, default) = args.get_one_two_args("os.getenv", heap)?;

    // Validate key is a string
    if key.is_str(heap) {
        // Build args to pass to host: (key, default)
        // The default is Value::None if not provided
        let final_default = default.unwrap_or(Value::None);
        let args = ArgValues::Two(key, final_default);

        Ok(AttrCallResult::OsCall(OsFunction::Getenv, args))
    } else {
        let type_name = key.py_type(heap);
        key.drop_with_heap(heap);
        if let Some(d) = default {
            d.drop_with_heap(heap);
        }
        Err(ExcType::type_error(format!("str expected, not {type_name}")))
    }
}
