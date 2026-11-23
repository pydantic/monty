//! Argument binding for the lazy map iterator.

use crate::{
    args::{ArgValues, FromArgs, LaxBool},
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::DropWithContext,
    types::map_filter::MapFilter,
    value::Value,
};

/// Binds `map(function, iterable, *iterables, strict=False)` without consuming any source.
pub fn builtin_map(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let MapArgs { mut iterables, strict } = MapArgs::from_args(args, vm)?;
    // Truth-test strict before reporting a missing source, as CPython does.
    let count = iterables.len();
    if count < 2 {
        iterables.drop_with(vm);
        return Err(ExcType::type_error(format_args!(
            "map() takes at least 2 arguments ({count} given)"
        )));
    }

    let function = iterables.remove(0);
    MapFilter::new_map(function, iterables, strict.bool(), vm)
}

/// Binds keywords before the custom positional-arity check and source acquisition.
#[derive(FromArgs)]
#[from_args(name = "map")]
struct MapArgs {
    #[from_args(varargs)]
    iterables: Vec<Value>,
    #[from_args(kw_only, default = LaxBool::new(false))]
    strict: LaxBool,
}
