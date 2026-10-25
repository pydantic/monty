//! Argument handling for `iter(object[, sentinel])`.

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::HeapData,
    types::callable_iterator::CallableIterator,
    value::Value,
};

/// Acquires an iterator or creates a callable iterator which stops at its sentinel.
pub(crate) fn builtin_iter(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let IterArgs { object, sentinel } = IterArgs::from_args(args, vm)?;
    if let Some(sentinel) = sentinel {
        if object.is_callable(vm) {
            let iter = CallableIterator::new(object, sentinel);
            Ok(CallResult::Value(Value::Ref(
                vm.heap.allocate(HeapData::CallableIterator(iter)),
            )))
        } else {
            object.drop_with(vm);
            sentinel.drop_with(vm);
            Err(ExcType::type_error("iter(v, w): v must be callable"))
        }
    } else {
        object.into_py_iter(vm)
    }
}

/// Argument shape for `iter(object, /)` and `iter(object, sentinel, /)`.
#[derive(FromArgs)]
#[from_args(name = "iter", style = unpack)]
struct IterArgs {
    #[from_args(pos_only)]
    object: Value,
    #[from_args(pos_only, default)]
    sentinel: Option<Value>,
}
