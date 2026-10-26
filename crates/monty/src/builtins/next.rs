//! Implementation of the next() builtin function.

use crate::{
    args::{ArgValues, FromArgs},
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    frozen::FrozenFunction,
    heap::{DropGuard, HeapReadOutput},
    os_dispatch::PostConversionEffect,
    types::PyTrait,
    value::Value,
};

/// Catches exhaustion after a user `__next__` call, including one that suspends.
pub(crate) const NEXT_DEFAULT_SOURCE: &str = r"
def __monty_next_default(iterator, default, next_, stop_iteration):
    try:
        return next_(iterator)
    except stop_iteration:
        return default
";

/// Argument shape for `next(iterator, default=...)` — positional-only in
/// CPython, so `style = unpack` gives the `PyArg_UnpackTuple` arity wording
/// and the blanket `next() takes no keyword arguments` rejection.
#[derive(FromArgs)]
#[from_args(name = "next", style = unpack)]
struct NextArgs {
    #[from_args(pos_only)]
    iterator: Value,
    #[from_args(pos_only, default)]
    default: Option<Value>,
}

/// Implementation of the next() builtin function.
///
/// Retrieves the next item from an iterator.
///
/// Two forms are supported:
/// - `next(iterator)` - Returns the next item from the iterator. Raises
///   `StopIteration` when the iterator is exhausted.
/// - `next(iterator, default)` - Returns the next item from the iterator, or
///   `default` if the iterator is exhausted.
///
/// A file nobody has read yet first loads its buffer from the host: the call
/// yields the read with a `FileNext` effect, which answers it on resume.
pub fn builtin_next(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let NextArgs { iterator, default } = NextArgs::from_args(args, vm)?;
    defer_drop!(iterator, vm);
    match iterator.read_heap(vm) {
        Some(HeapReadOutput::Instance(instance)) => {
            if let Some(default) = default {
                vm.call_frozen_exact(
                    FrozenFunction::NextDefault,
                    [
                        iterator.clone_with_heap(vm.heap),
                        default,
                        Value::Builtin(Builtins::Function(BuiltinsFunctions::Next)),
                        Value::Builtin(Builtins::ExcType(ExcType::StopIteration)),
                    ],
                )
            } else {
                instance.call_next(vm)
            }
        }
        Some(HeapReadOutput::OpenFile(file)) if file.needs_buffer_load(vm) => {
            let effect = PostConversionEffect::FileNext {
                file_id: file.id(),
                default,
            };
            Ok(file.buffer_load_call(vm, effect))
        }
        _ => next_native(iterator, default, vm).map(CallResult::Value),
    }
}

/// Advances a native iterator without allocating a continuation for its optional default.
fn next_native(iterator: &Value, default: Option<Value>, vm: &mut VM<'_>) -> RunResult<Value> {
    let mut default_guard = DropGuard::new(default, vm);
    let vm = default_guard.ctx();
    let Value::Ref(id) = iterator else {
        return Err(ExcType::type_error_not_iterator(&iterator.py_type_name(vm)));
    };
    match vm.heap.read(*id).py_next(vm)? {
        Some(item) => Ok(item),
        None => match default_guard.into_inner() {
            Some(default) => Ok(default),
            None => Err(ExcType::stop_iteration()),
        },
    }
}
