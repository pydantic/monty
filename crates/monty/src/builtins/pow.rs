//! Implementation of the pow() builtin function.

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::RunResult,
    value::Value,
};

/// Implementation of the pow() builtin function.
///
/// Dispatches `__pow__` / `__rpow__`, passing an optional modulus unchanged.
/// Builtin numeric types accept a modulus only for integer operands.
pub fn builtin_pow(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let PowArgs { base, exp, modulus } = PowArgs::from_args(args, vm)?;
    defer_drop!(base, vm);
    defer_drop!(exp, vm);
    defer_drop!(modulus, vm);
    base.py_pow(exp, Some(modulus), vm)
}

/// `pow(base, exp[, mod])` — CPython accepts all three as positional-or-keyword
/// (and `mod` defaults to `None`), but Monty has not plumbed kwarg dispatch
/// through to the dispatch body yet. `kwargs_not_supported_yet` rejects
/// any kwarg with `NotImplementedError: pow() does not yet support
/// keyword arguments` (replacing the previous `TypeError: pow() takes no
/// keyword arguments` from `into_pos_only`) while the macro takes over
/// positional arity validation — the bespoke
/// `pow expected 2 or 3 arguments, got N` message becomes CPython's
/// `pow() takes at most 3 arguments (N given)` /
/// `pow() missing required argument 'X' (pos N)`. The `modulus` field
/// will be renamed to `r#mod` and lose the flag when kwargs are
/// implemented.
#[derive(FromArgs)]
#[from_args(name = "pow", style = c_named, at_most_total, kwargs_not_supported_yet)]
struct PowArgs {
    base: Value,
    exp: Value,
    #[from_args(default = Value::None)]
    modulus: Value,
}
