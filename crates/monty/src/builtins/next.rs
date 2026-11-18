//! Implementation of the next() builtin function.

use crate::{
    args::{ArgValues, FromArgs},
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, VM},
    defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    frozen::FrozenFunction,
    types::{PyTrait, Type},
    value::Value,
};

/// Catches exhaustion after any iterator advancement, including one that suspends.
pub(crate) const NEXT_DEFAULT_SOURCE: &str = r"
def __monty_next_default(iterator, default, next_, stop_iteration):
    try:
        return next_(iterator)
    except stop_iteration:
        return default
";

/// Positional-only `next(iterator[, default])`, preserving an explicitly supplied `None`.
#[derive(FromArgs)]
#[from_args(name = "next", style = unpack)]
struct NextArgs {
    #[from_args(pos_only)]
    iterator: Value,
    #[from_args(pos_only, default)]
    default: Option<Value>,
}

/// Advances an iterator, optionally replacing exhaustion with a default after resumption.
pub fn builtin_next(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let NextArgs { iterator, default } = NextArgs::from_args(args, vm)?;
    defer_drop_mut!(iterator, vm);
    iterator.py_next_call(default, vm)?.ok_or_else(ExcType::stop_iteration)
}

/// Wraps a potentially suspending advancement so exhaustion can select its owned default.
pub(crate) fn call_with_default(iterator: Value, default: Value, vm: &mut VM<'_>) -> RunResult<CallResult> {
    vm.call_frozen_exact(
        FrozenFunction::NextDefault,
        [
            iterator,
            default,
            Value::Builtin(Builtins::Function(BuiltinsFunctions::Next)),
            Value::Builtin(Builtins::Type(Type::StopIteration)),
        ],
    )
}
