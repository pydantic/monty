//! Runtime behaviour for `collections.defaultdict`.
//!
//! A defaultdict is a `dict` tagged with a defaultdict [`DictKind`](crate::types::DictKind),
//! carrying its `default_factory`. The only behavioural addition is the
//! missing-key hook below; everything else is the inherited dict surface.

use crate::{
    args::ArgValues,
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapId, HeapReadOutput},
    value::Value,
};

/// Implements `defaultdict.__missing__(key)` — the hook a missing-key access
/// invokes: if a `default_factory` is set, call it, insert `key -> factory()`,
/// and return the value; otherwise raise `KeyError`.
///
/// Shared by the getitem miss path (`Value::py_getitem`) and the explicit
/// `__missing__` method. The factory runs via `VM::evaluate_function`, so a
/// factory that performs external/OS calls is unsupported (documented).
pub(crate) fn defaultdict_missing(dict_id: HeapId, key: &Value, vm: &mut VM<'_>) -> RunResult<Value> {
    let factory = match vm.heap.get(dict_id) {
        HeapData::Dict(d) => d.default_factory().map(|f| f.clone_with_heap(vm.heap)),
        _ => unreachable!("defaultdict_missing on a non-dict heap entry"),
    };
    let Some(factory) = factory else {
        return Err(ExcType::key_error(key, vm));
    };
    let value = match vm.evaluate_function("default_factory", &factory, ArgValues::Empty) {
        Ok(value) => value,
        Err(e) => {
            factory.drop_with(vm);
            return Err(e);
        }
    };
    factory.drop_with(vm);

    // Insert `key -> value` and return the same value (CPython stores and
    // returns the one object the factory produced).
    let key_clone = key.clone_with_heap(vm.heap);
    let value_ret = value.clone_with_heap(vm.heap);
    let HeapReadOutput::Dict(mut dict) = vm.heap.read(dict_id) else {
        unreachable!("defaultdict_missing on a non-dict heap entry");
    };
    // A `set` failure (e.g. a key `__eq__` raising during collision) must
    // release the cloned `value_ret`, which the `?` early-return would leak.
    match dict.set(key_clone, value, vm) {
        Ok(Some(old)) => old.drop_with(vm),
        Ok(None) => {}
        Err(e) => {
            value_ret.drop_with(vm);
            return Err(e);
        }
    }
    Ok(value_ret)
}
