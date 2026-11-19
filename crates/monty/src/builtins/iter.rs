//! Argument handling for `iter(object[, sentinel])`.

use monty_macros::py_signature;

use crate::{
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    types::callable_iterator::CallableIterator,
    value::Value,
};

/// Acquires an iterator or creates a callable iterator which stops at its sentinel.
#[py_signature("iter(object, sentinel=..., /)")]
pub(crate) fn builtin_iter(vm: &mut VM<'_>, params: &mut IterArgs) -> RunResult<CallResult> {
    let IterArgsFields { object, sentinel } = params.take_all();
    if let Some(sentinel) = sentinel {
        if object.is_callable(vm) {
            let iter = CallableIterator::new(object, sentinel);
            Ok(CallResult::Value(Value::allocate(iter, vm.heap)))
        } else {
            object.drop_with(vm);
            sentinel.drop_with(vm);
            Err(ExcType::type_error("iter(v, w): v must be callable"))
        }
    } else {
        object.into_py_iter(vm)
    }
}
