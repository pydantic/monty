//! Implementation of the pow() builtin function.

use num_bigint::BigInt;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    defer_drop,
    exception_private::{ExcType, ExcTypeExt, RunResult, SimpleException},
    heap::{HeapData, Reference, RuntimeRef},
    types::{Type, long_int::modular_pow},
    value::Value,
};

/// Implementation of the pow() builtin function.
///
/// Returns base to the power exp. With three arguments, returns (base ** exp) % mod,
/// which is only defined for integers.
pub fn builtin_pow<H: RuntimeRef>(vm: &mut VM<'_, H>, args: ArgValues<H>) -> RunResult<Value<H>> {
    let PowArgs { base, exp, modulus } = PowArgs::from_args(args, vm)?;
    defer_drop!(base, vm);
    defer_drop!(exp, vm);
    defer_drop!(modulus, vm);
    let integers = [Value::Int(0), Value::Int(1)];
    let int_base = normalize_bool(base, &integers);
    let int_exp = normalize_bool(exp, &integers);

    match modulus {
        // `pow(base, exp)` is `base ** exp`: the operator already covers every numeric pairing.
        Value::None => int_base.py_pow(int_exp, None, vm),
        modulus => {
            let int_modulus = normalize_bool(modulus, &integers);
            let result = match int_base {
                Value::Int(base) => modular_pow(&BigInt::from(*base), int_exp, int_modulus, vm.heap)?,
                Value::Ref(id) if let HeapData::LongInt(base) = vm.heap.get(*id) => {
                    modular_pow(base.inner(), int_exp, int_modulus, vm.heap)?
                }
                _ => None,
            };
            result.ok_or_else(|| {
                // CPython offers the triple to each operand's `__pow__` in turn: a float
                // refuses the third argument outright, a complex does so only once both
                // base and exponent are numbers, and any other mix is an unsupported triple.
                let is_complex = |value: &Value<H>| matches!(value.py_type_heap(vm.heap), Type::Complex);
                let numeric = |value: &Value<H>| {
                    matches!(
                        value.py_type_heap(vm.heap),
                        Type::Int | Type::Bool | Type::Float | Type::Complex
                    )
                };
                let refusal = [base, exp, modulus].into_iter().find_map(|value| match value {
                    Value::Float(_) => Some(
                        SimpleException::new_msg(
                            ExcType::TypeError,
                            "pow() 3rd argument not allowed unless all arguments are integers",
                        )
                        .into(),
                    ),
                    _ if is_complex(value) && numeric(base) && numeric(exp) => {
                        Some(ExcType::value_error_complex_modulo())
                    }
                    _ => None,
                });
                refusal.unwrap_or_else(|| {
                    ExcType::ternary_pow_type_error(
                        base.py_type_name(vm),
                        exp.py_type_name(vm),
                        modulus.py_type_name(vm),
                    )
                })
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
struct PowArgs<H: Reference> {
    base: Value<H>,
    exp: Value<H>,
    #[from_args(default = Value::None)]
    modulus: Value<H>,
}

/// Normalizes a `Bool` to its `Int` equivalent by reference.
///
/// Uses caller-owned integer values so the original type remains available for errors.
fn normalize_bool<'a, H: RuntimeRef>(value: &'a Value<H>, integers: &'a [Value<H>; 2]) -> &'a Value<H> {
    match value {
        Value::Bool(false) => &integers[0],
        Value::Bool(true) => &integers[1],
        other => other,
    }
}
