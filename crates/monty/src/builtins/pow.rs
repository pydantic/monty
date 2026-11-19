//! Implementation of the pow() builtin function.

use monty_macros::py_signature;

use crate::{bytecode::VM, exception_private::RunResult, value::Value};

/// Implementation of the pow() builtin function.
///
/// Dispatches `__pow__` / `__rpow__`, passing an optional modulus unchanged.
/// Builtin numeric types accept a modulus only for integer operands.
///
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
#[py_signature("pow(base, exp, modulus=None)", kwargs_not_supported_yet)]
pub fn builtin_pow(vm: &mut VM<'_>, args: &PowArgs) -> RunResult<Value> {
    let PowArgs { base, exp, modulus } = args;
    base.py_pow(exp, Some(modulus), vm)
}
