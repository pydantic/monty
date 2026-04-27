//! Implementation of Python's `gc` module — only available under the `test-hooks` feature.
//!
//! This module exists purely so integration tests can drive Monty's garbage
//! collector deterministically from Python source. It is **not** part of the
//! public sandbox surface: enabling it from production builds would let
//! untrusted code force GC cycles, which is not a behavior we want exposed.
//!
//! The only function provided is `gc.collect()`, which forces a full GC cycle
//! using the production root walk (the same one the VM runs implicitly when
//! `should_gc()` fires). It returns `0` to mirror CPython's signature
//! (number of unreachable objects collected) without us needing to track that
//! statistic — Monty's GC frees unreachable objects but doesn't surface a count.

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
/// Currently only `collect` is implemented — that is sufficient to let tests
/// trigger a deterministic GC cycle from Python without reaching into Rust
/// helpers like `VM::__force_gc_for_tests`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum GcFunctions {
    /// `gc.collect()` — forces a full garbage collection cycle.
    Collect,
}

/// Creates the `gc` module and allocates it on the heap.
///
/// Registers `gc.collect` as a `ModuleFunctions::Gc` variant. The module is
/// otherwise empty — we deliberately do not expose CPython's tuning knobs
/// (`gc.disable`, `gc.set_threshold`, `gc.get_objects`, ...) because they
/// would let test code reach into and observe the heap in ways that aren't
/// stable across Monty versions.
pub fn create_module(vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Gc);
    module.set_attr(
        StaticStrings::Collect,
        Value::ModuleFunction(ModuleFunctions::Gc(GcFunctions::Collect)),
        vm,
    );
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
    }
}

/// `gc.collect()` — forces a full GC cycle and returns `0`.
///
/// Mirrors CPython's signature (which returns the number of unreachable
/// objects collected), but always returns `0` because Monty's GC doesn't
/// track that count. Tests that need to assert specific GC behavior should
/// observe the side effects (e.g. that previously-unreachable cycles are
/// freed) rather than the return value.
fn collect(vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    args.check_zero_args("gc.collect", vm.heap)?;
    vm.__force_gc_for_tests();
    Ok(Value::Int(0))
}
