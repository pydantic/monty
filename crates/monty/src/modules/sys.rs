//! Implementation of the `sys` module.
//!
//! Provides a minimal implementation of Python's `sys` module with:
//! - `version`: Python version string (e.g., "3.14.0 (Monty)")
//! - `version_info`: Named tuple (3, 14, 0, 'final', 0)
//! - `platform`: Platform identifier ("monty")
//! - `stdout`: Marker for standard output (no real functionality)
//! - `stderr`: Marker for standard error (no real functionality)
//!
//! Under the `test-hooks` feature one callable is also exposed:
//! - `setrecursionlimit(n)`: tighten the active recursion ceiling so fixtures
//!   can simulate Monty's lower default depth on CPython too. Only allows
//!   *lowering* the host-configured ceiling — see [`SysFunctions`].

#[cfg(feature = "test-hooks")]
use crate::{
    args::ArgValues,
    exception_private::{ExcType, RunResult},
    modules::ModuleFunctions,
};
use crate::{
    bytecode::VM,
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker},
    types::{Module, NamedTuple},
    value::{Marker, Value},
};

/// Functions exposed by the `sys` module under the `test-hooks` feature.
///
/// Production builds keep `sys` attribute-only; this enum exists so fixtures
/// (and only fixtures) can call back into Monty internals that production
/// sandbox code must never reach.
#[cfg(feature = "test-hooks")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum SysFunctions {
    /// `sys.setrecursionlimit(n)` — tightens the live recursion ceiling to
    /// `n`. Only allows lowering; attempting to raise raises `ValueError`.
    Setrecursionlimit,
}

/// Creates the `sys` module and allocates it on the heap.
///
/// Returns a HeapId pointing to the newly allocated module.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Sys);

    // sys.platform
    module.set_attr(StaticStrings::Platform, StaticStrings::Monty.into(), vm);

    // sys.stdout / sys.stderr - markers for standard output/error
    module.set_attr(StaticStrings::Stdout, Value::Marker(Marker(StaticStrings::Stdout)), vm);
    module.set_attr(StaticStrings::Stderr, Value::Marker(Marker(StaticStrings::Stderr)), vm);

    // sys.version
    module.set_attr(StaticStrings::Version, StaticStrings::MontyVersionString.into(), vm);
    // sys.version_info - named tuple (major=3, minor=14, micro=0, releaselevel='final', serial=0)
    let version_info = NamedTuple::new(
        StaticStrings::SysVersionInfo,
        vec![
            StaticStrings::Major.into(),
            StaticStrings::Minor.into(),
            StaticStrings::Micro.into(),
            StaticStrings::Releaselevel.into(),
            StaticStrings::Serial.into(),
        ],
        vec![
            Value::Int(3),
            Value::Int(14),
            Value::Int(0),
            Value::InternString(StaticStrings::Final.into()),
            Value::Int(0),
        ],
    );
    let version_info_id = vm.heap.allocate(HeapData::NamedTuple(version_info))?;
    module.set_attr(StaticStrings::VersionInfo, Value::Ref(version_info_id), vm);

    // Test-only callables — see the module-level docs and the
    // [`test-hooks`] feature gate.
    #[cfg(feature = "test-hooks")]
    module.set_attr(
        StaticStrings::Setrecursionlimit,
        Value::ModuleFunction(ModuleFunctions::Sys(SysFunctions::Setrecursionlimit)),
        vm,
    );

    vm.heap.allocate(HeapData::Module(module))
}

/// Dispatches a `sys` module function call.
///
/// Only present under the `test-hooks` feature — production builds register
/// no callables on the `sys` module, so this dispatcher would have nothing
/// to do.
#[cfg(feature = "test-hooks")]
pub(super) fn call(vm: &mut VM<'_, impl ResourceTracker>, function: SysFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        SysFunctions::Setrecursionlimit => setrecursionlimit(vm, args),
    }
}

/// `sys.setrecursionlimit(n)` — tightens the live recursion ceiling.
///
/// Differs from CPython in one safety-critical way: the limit may only be
/// *lowered*, never raised. Sandboxed code raising the ceiling would let it
/// escape the host-configured depth bound that protects the Rust call stack
/// from overflow inside recursive type machinery (repr, eq, hash, json
/// dump, etc.). Attempts to raise raise `ValueError` with a message
/// pointing at the current cap.
#[cfg(feature = "test-hooks")]
fn setrecursionlimit(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let arg = args.get_one_arg("sys.setrecursionlimit", vm.heap)?;
    let Value::Int(limit) = arg else {
        arg.drop_with_heap(vm);
        return Err(ExcType::type_error("sys.setrecursionlimit() argument must be int"));
    };
    let Ok(new_limit) = usize::try_from(limit) else {
        return Err(ExcType::value_error("recursion limit must be greater or equal than 1"));
    };
    if new_limit == 0 {
        return Err(ExcType::value_error("recursion limit must be greater or equal than 1"));
    }
    match vm.heap.tracker().lower_recursion_limit(new_limit) {
        Ok(()) => Ok(Value::None),
        Err(Some(current)) => Err(ExcType::value_error(format!(
            "sys.setrecursionlimit: cannot raise above current limit {current} (sandbox only allows lowering)"
        ))),
        Err(None) => Err(ExcType::value_error(
            "sys.setrecursionlimit: this runtime does not expose a settable recursion limit",
        )),
    }
}
