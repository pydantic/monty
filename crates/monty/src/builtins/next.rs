//! Implementation of the next() builtin function.

use crate::{
    args::{ArgValues, FromArgs},
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, VM},
    defer_drop_mut,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    frozen::FrozenFunction,
    heap::DropGuard,
    py_value::PyValue,
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
    let Some(default) = default else {
        defer_drop_mut!(iterator, vm);
        return iterator.py_next(vm)?.ok_or_else(ExcType::stop_iteration);
    };
    // Only bypass the handler for sources known not to call Python. All other iterators
    // get the handler before dispatch, even if they currently always finish inline.
    if let Some(result) = PyValue::native_iterator(&iterator, vm).map(|mut native| native.next_sync(vm)) {
        iterator.drop_with(vm);
        let default = DropGuard::new(default, vm);
        return Ok(CallResult::Value(result?.unwrap_or_else(|| default.into_inner())));
    }
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
