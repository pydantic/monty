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
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    exception_private::RunResult,
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
/// Parsing goes through a small `FromArgs`-derived struct so type validation
/// (key must be str) and arity checks (1-or-2 args) use the same machinery
/// as the rest of the codebase; the `default` field is then snapshotted into
/// a [`MontyObject`] so it can travel with the OS call across the boundary.
fn getenv(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
    let GetenvParseArgs { key, default } = GetenvParseArgs::from_args(args, vm.heap, vm.interns)?;
    let default = MontyObject::new(default, vm);
    Ok(CallResult::OsCall(OsFunctionCall::Getenv(GetenvArgs { key, default })))
}

/// `FromArgs`-side shape for `os.getenv`. Distinct from [`GetenvArgs`] (the
/// OS-call payload) because `default` is parsed as a raw `Value` here and
/// then projected into a `MontyObject` for the payload — a projection that
/// needs `MontyObject::new(value, vm)` and therefore can't be expressed
/// through `FromArgs` alone today (the `FromValue` trait surface only has
/// `&mut Heap + &Interns`, not the `HeapReader` + interns that
/// `MontyObject::new` needs to walk container heap entries).
#[derive(FromArgs)]
#[from_args(name = "os.getenv", bad_arg_named)]
struct GetenvParseArgs {
    key: String,
    #[from_args(default = Value::None)]
    default: Value,
}
