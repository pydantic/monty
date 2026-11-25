//! Argument binding for the lazy map iterator.

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::{CallResult, VM},
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{DropGuard, DropWithContext},
    types::{PyTrait, map_filter::MapFilter},
    value::Value,
};

/// Binds `map(function, iterable, *iterables, strict=False)` without consuming any source.
pub fn builtin_map(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let keyword_count = match &args {
        ArgValues::Kwargs(kwargs) | ArgValues::ArgsKargs { kwargs, .. } => kwargs.len(),
        _ => 0,
    };
    if keyword_count > 1 {
        args.drop_with(vm);
        return Err(ExcType::type_error_method_at_most("map", 1, keyword_count, true));
    }
    let MapArgs { args, strict } = MapArgs::from_args(args, vm)?;
    let mut sources = DropGuard::new(args, vm);
    let strict = {
        let vm = sources.ctx();
        defer_drop!(strict, vm);
        strict.py_bool(vm)?
    };
    // CPython validates keywords and truth-tests strict before its bespoke positional arity check.
    if sources.as_parts().0.len() < 2 {
        return Err(ExcType::type_error_map_arity());
    }
    let (mut iterables, vm) = sources.into_parts();
    let function = iterables.remove(0);
    MapFilter::new_map(function, iterables, strict, vm)
}

/// Binds keywords before the custom positional-arity check and source acquisition.
#[derive(FromArgs)]
#[from_args(name = "map")]
struct MapArgs {
    #[from_args(varargs)]
    args: Vec<Value>,
    #[from_args(kw_only, default = Value::Bool(false))]
    strict: Value,
}
