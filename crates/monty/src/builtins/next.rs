//! Implementation of the next() builtin function.

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    resource::ResourceTracker,
    types::iter::iterator_next,
    value::Value,
};

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
pub fn builtin_next(vm: &mut VM<'_, impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let NextArgs { iterator, default } = NextArgs::from_args(args, vm)?;
    defer_drop!(iterator, vm);
    iterator_next(iterator, default, vm)
}
