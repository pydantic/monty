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
    MontyObject,
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunResult},
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    os::{GetenvArgs, OsFunctionCall},
    resource::{ResourceError, ResourceTracker},
    types::{Module, Property, property::ZeroArgOsProperty},
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
pub fn create_module(vm: &mut VM<'_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Os);

    // os.getenv - function to get a single environment variable
    module.set_attr(
        StaticStrings::Getenv,
        Value::ModuleFunction(ModuleFunctions::Os(OsFunctions::Getenv)),
        vm,
    );

    // os.environ - property that returns the entire environment as a dict
    module.set_attr(
        StaticStrings::Environ,
        Value::Property(Property::Os(ZeroArgOsProperty::GetEnviron)),
        vm,
    );

    vm.heap.allocate(HeapData::Module(module))
}

/// Dispatches a call to an os module function.
///
/// Returns `CallResult::OsCall` for functions that need host involvement,
/// or `CallResult::Value` for functions that can be computed immediately.
pub(super) fn call(
    vm: &mut VM<'_, impl ResourceTracker>,
    functions: OsFunctions,
    args: ArgValues,
) -> RunResult<CallResult> {
    match functions {
        OsFunctions::Getenv => getenv(vm, args),
    }
}

/// Implementation of `os.getenv(key, default=None)`.
///
/// Hand-rolled rather than `FromArgs`-derived so the `key`-must-be-`str`
/// error matches CPython's wording exactly — CPython routes `os.getenv`
/// through `os.environ.__getitem__`, whose `check_str` helper raises
/// `TypeError("str expected, not <type>")`. That wording is bespoke to
/// `os._Environ` in the CPython stdlib, so it lives inline here rather
/// than as a shared helper.
fn getenv(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
    let (key_value, default_value) = args.get_one_two_args("os.getenv", vm.heap)?;
    if let Some(key) = key_value.as_either_str(vm.heap) {
        key_value.drop_with_heap(vm.heap);
        Ok(CallResult::OsCall(OsFunctionCall::Getenv(GetenvArgs {
            key: key.into_string(vm.interns),
            default: MontyObject::new(default_value.unwrap_or(Value::None), vm),
        })))
    } else {
        let type_name = key_value.py_type_heap(vm.heap);
        key_value.drop_with_heap(vm.heap);
        if let Some(d) = default_value {
            d.drop_with_heap(vm.heap);
        }
        Err(ExcType::type_error(format!("str expected, not {type_name}")))
    }
}
