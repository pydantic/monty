//! Argument binding for the lazy map iterator.

use monty_macros::py_signature;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::DropWithContext,
    types::map_filter::MapFilter,
};

/// Binds `map(function, iterable, *iterables, strict=False)` without consuming any source.
pub fn builtin_map(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let MapArgs { mut iterables, strict } = MapArgs::from_args(args, vm)?;
    // Truth-test strict before reporting a missing source, as CPython does.
    let count = iterables.len();
    if count < 2 {
        iterables.drop_with(vm);
        return Err(ExcType::type_error_map_arity());
    }

    let function = iterables.remove(0);
    MapFilter::new_map(function, iterables, strict.bool(), vm)
}

/// Binds keywords before the custom positional-arity check and source acquisition.
#[py_signature("map(*iterables, strict: bool=False)")]
struct MapArgs;
