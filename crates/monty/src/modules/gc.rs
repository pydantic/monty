//! Implementation of Python's `gc` module — only available under the `test-hooks` feature.
//!
//! This module exists purely so integration tests and benches can drive Monty's
//! garbage collector deterministically from Python source. It is **not** part of
//! the public sandbox surface: enabling it from production builds would let
//! untrusted code force GC cycles or suppress them entirely, which is not a
//! behavior we want exposed.
//!
//! The functions provided are:
//! - `gc.collect()` — forces a full GC cycle using the production root walk
//!   (the same one the VM runs implicitly when `should_gc()` fires). Returns the
//!   number of unreachable heap entries that were freed during the sweep.
//! - `gc.disable()` / `gc.enable()` — toggle automatic collection.
//!   Explicit `gc.collect()` calls still run while disabled.

use crate::{
    args::ArgValues,
    bytecode::VM,
    exception_private::RunResult,
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    resource::{ResourceError, ResourceTracker},
    types::Module,
    value::Value,
};

/// Functions exposed by the `gc` module.
///
/// Just enough surface for tests and benches to script deterministic GC cycles
/// from Python — no observation/tuning APIs (`gc.get_objects`, `gc.set_threshold`,
/// ...) since those would let test code peer at heap internals in ways that
/// aren't stable across Monty versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum GcFunctions {
    /// `gc.collect()` — forces a full garbage collection cycle.
    Collect,
    /// `gc.disable()` — suppresses automatic GC until `gc.enable()` is called.
    Disable,
    /// `gc.enable()` — resumes automatic GC after a prior `gc.disable()`.
    Enable,
}

/// Creates the `gc` module and allocates it on the heap.
pub fn create_module(vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Gc);
    for (name, function) in [
        (StaticStrings::Collect, GcFunctions::Collect),
        (StaticStrings::Disable, GcFunctions::Disable),
        (StaticStrings::Enable, GcFunctions::Enable),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Gc(function)), vm);
    }
    vm.heap.allocate(HeapData::Module(module))
}

/// Dispatches a `gc` module function call.
///
/// Returns `Value` directly because none of the exposed functions need host
/// involvement — they all run synchronously inside the VM.
pub(super) fn call(
    vm: &mut VM<'_, '_, impl ResourceTracker>,
    function: GcFunctions,
    args: ArgValues,
) -> RunResult<Value> {
    match function {
        GcFunctions::Collect => collect(vm, args),
        GcFunctions::Disable => disable(vm, args),
        GcFunctions::Enable => enable(vm, args),
    }
}

/// `gc.collect()` — forces a full GC cycle and returns the number of unreachable
/// heap entries freed during the sweep.
///
/// CPython returns the count of *unreachable objects* found, which is the same
/// number Monty's mark-sweep frees in one pass — every unreachable entry is
/// reclaimed (Monty has no equivalent of CPython's `gc.garbage` for objects with
/// finalizers, so nothing survives the sweep).
///
/// Counts are clamped to `i64::MAX` if they ever exceed it; in practice a single
/// heap can't hold that many entries, but the conversion is fallible so we
/// saturate rather than panic.
fn collect(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    args.check_zero_args("gc.collect", vm.heap)?;
    let freed = vm.__force_gc_for_tests();
    Ok(Value::Int(i64::try_from(freed).unwrap_or(i64::MAX)))
}

/// `gc.disable()` — suppresses automatic GC until [`enable`] is called.
///
/// Returns `None` to match CPython. Explicit [`collect`] calls still run while
/// auto-GC is disabled, so a script can build a known amount of garbage and then
/// time exactly one collection pass.
fn disable(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    args.check_zero_args("gc.disable", vm.heap)?;
    vm.heap.disable_gc();
    Ok(Value::None)
}

/// `gc.enable()` — re-enables automatic GC after a prior [`disable`].
///
/// Returns `None` to match CPython. Calling on an already-enabled heap is a no-op.
fn enable(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    args.check_zero_args("gc.enable", vm.heap)?;
    vm.heap.enable_gc();
    Ok(Value::None)
}
