//! Implementation of the pow() builtin function.

use num_bigint::BigInt;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    heap::HeapData,
    types::long_int::modular_pow,
    value::Value,
};

/// Implementation of the pow() builtin function.
///
/// Returns base to the power exp. With three arguments, returns (base ** exp) % mod,
/// which is only defined for integers.
pub fn builtin_pow(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let PowArgs { base, exp, modulus } = PowArgs::from_args(args, vm)?;
    defer_drop!(base, vm);
    defer_drop!(exp, vm);
    defer_drop!(modulus, vm);
    let int_base = normalize_bool(base);
    let int_exp = normalize_bool(exp);

    match modulus {
        // `pow(base, exp)` is `base ** exp`: the operator already covers every numeric pairing.
        Value::None => int_base.py_pow(int_exp, None, vm),
        modulus => {
            let int_modulus = normalize_bool(modulus);
            let result = match int_base {
                Value::Int(base) => modular_pow(&BigInt::from(*base), int_exp, int_modulus, vm.heap)?,
                Value::Ref(id) if let HeapData::LongInt(base) = vm.heap.get(*id) => {
                    modular_pow(base.inner(), int_exp, int_modulus, vm.heap)?
                }
                _ => None,
            };
            result.ok_or_else(|| {
                // A float operand refuses the third argument outright, as `float.__pow__` does;
                // any other mix is reported as an unsupported operand triple.
                if [base, exp, modulus]
                    .iter()
                    .any(|value| matches!(value, Value::Float(_)))
                {
                    SimpleException::new_msg(
                        ExcType::TypeError,
                        "pow() 3rd argument not allowed unless all arguments are integers",
                    )
                    .into()
                } else {
                    ExcType::ternary_pow_type_error(
                        base.py_type_name(vm),
                        exp.py_type_name(vm),
                        modulus.py_type_name(vm),
                    )
                }
            })
        }
    }
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

/// Normalizes a `Bool` to its `Int` equivalent by reference.
///
/// Returns `&Value::Int(0)` or `&Value::Int(1)` for bools (using static storage),
/// and the original reference unchanged for all other types.
fn normalize_bool(value: &Value) -> &Value {
    static FALSE_INT: Value = Value::Int(0);
    static TRUE_INT: Value = Value::Int(1);
    match value {
        Value::Bool(false) => &FALSE_INT,
        Value::Bool(true) => &TRUE_INT,
        other => other,
    }
}
