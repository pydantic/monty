//! Implementation of Python's `itertools` module.
//!
//! Only `count(start=0, step=1)` and `repeat(object, times=?)` so far; every
//! other name is absent from the namespace, so it raises `AttributeError`
//! rather than failing later. See `limitations/itertools.md`, and
//! [`crate::types::itertools`] for why the family shares one `HeapData` variant.

use monty_types::ResourceError;

use crate::{
    args::{ArgValues, FromArgs},
    bytecode::VM,
    exception_private::{ExcType, ExcTypeExt, RunResult},
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{
        ItertoolsIter, Module, Type,
        itertools::{Count, Repeat},
    },
    value::Value,
};

/// `itertools` module functions — each variant is a Python-visible callable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum ItertoolsFunctions {
    Count,
    Repeat,
}

/// Static mapping of attribute names to functions for module creation.
const ITERTOOLS_FUNCTIONS: &[(StaticStrings, ItertoolsFunctions)] = &[
    (StaticStrings::Count, ItertoolsFunctions::Count),
    (StaticStrings::Repeat, ItertoolsFunctions::Repeat),
];

/// Creates the `itertools` module on the heap.
///
/// # Panics
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Itertools);

    for (name, func) in ITERTOOLS_FUNCTIONS {
        module.set_attr(*name, Value::ModuleFunction(ModuleFunctions::Itertools(*func)), vm);
    }

    vm.heap.allocate(HeapData::Module(module))
}

/// Dispatches a call to an `itertools` module function.
pub(super) fn call(vm: &mut VM<'_>, function: ItertoolsFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        ItertoolsFunctions::Count => call_count(vm, args),
        ItertoolsFunctions::Repeat => call_repeat(vm, args),
    }
}

/// Argument shape for `count(start=0, step=1)`.
///
/// CPython parses this with `"|OO:count"`, so errors carry the function name
/// (`style = c_named`) and arity counts positionals + keywords together
/// (`at_most_total`): `count(1, 2, step=3)` reports three arguments.
#[derive(FromArgs)]
#[from_args(name = "count", style = c_named, at_most_total)]
struct CountArgs {
    #[from_args(default = Value::Int(0))]
    start: Value,
    #[from_args(default = Value::Int(1))]
    step: Value,
}

/// `itertools.count(start=0, step=1)` — an infinite arithmetic progression.
fn call_count(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let CountArgs { start, step } = CountArgs::from_args(args, vm)?;
    // CPython's `count_new` runs one `PyNumber_Check` over both arguments after
    // parsing, so a bad `start` and a bad `step` give the same single message.
    // Rejecting here means `py_next` can add without a defined-ness check.
    if is_number(&start, vm) && is_number(&step, vm) {
        let iter = ItertoolsIter::Count(Count::new(normalize_bool(start), normalize_bool(step)));
        Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(Box::new(iter)))?))
    } else {
        start.drop_with(vm);
        step.drop_with(vm);
        Err(ExcType::type_error("a number is required"))
    }
}

/// Argument shape for `repeat(object, times=?)`.
///
/// CPython parses this with `"O|n:repeat"` — see [`CountArgs`] for why that
/// means `c_named` + `at_most_total`. `times` stays a raw `Value` so the
/// `__index__` coercion (and its `OverflowError`) happens in the body.
#[derive(FromArgs)]
#[from_args(name = "repeat", style = c_named, at_most_total)]
struct RepeatArgs {
    object: Value,
    #[from_args(default)]
    times: Option<Value>,
}

/// `itertools.repeat(object, times=?)` — `object` forever, or `times` times.
fn call_repeat(vm: &mut VM<'_>, args: ArgValues) -> RunResult<Value> {
    let RepeatArgs { object, times } = RepeatArgs::from_args(args, vm)?;
    let remaining = match times {
        None => None,
        Some(times) => {
            let count = repeat_times(&times, vm);
            times.drop_with(vm);
            match count {
                Ok(count) => Some(count),
                Err(error) => {
                    object.drop_with(vm);
                    return Err(error);
                }
            }
        }
    };
    let iter = ItertoolsIter::Repeat(Repeat::new(object, remaining));
    Ok(Value::Ref(vm.heap.allocate(HeapData::Itertools(Box::new(iter)))?))
}

/// Whether `value` satisfies CPython's `PyNumber_Check` for `count()`.
///
/// Monty's numeric types are exactly `int` (immediate, interned big, or heap
/// `LongInt` — all reported as [`Type::Int`]), `float`, and `bool`.
fn is_number(value: &Value, vm: &VM<'_>) -> bool {
    matches!(value.py_type_heap(vm.heap), Type::Int | Type::Float | Type::Bool)
}

/// Widens a `bool` start/step to the `int` it stands for.
///
/// CPython's `count_new` does the same — `repr(count(True))` is `count(1)`, not
/// `count(True)` — so this is parity rather than a shortcut. It also keeps
/// `py_next` off Monty's unsupported `bool + int` path.
fn normalize_bool(value: Value) -> Value {
    match value {
        Value::Bool(b) => Value::Int(i64::from(b)),
        other => other,
    }
}

/// Coerces `repeat`'s `times` the way CPython's `n` format unit does.
///
/// Negative counts clamp to zero (`repeat(x, -1)` is empty) and a `times` too
/// large for a machine integer raises `OverflowError`, matching the conversion
/// to `Py_ssize_t`. `bool` is accepted because it is an `int` subclass.
fn repeat_times(value: &Value, vm: &VM<'_>) -> RunResult<usize> {
    let count = match value {
        Value::Bool(b) => i64::from(*b),
        other => other.as_int(vm)?,
    };
    // Saturates rather than wrapping on a 32-bit host, where a `times` between
    // `usize::MAX` and `i64::MAX` is still effectively infinite.
    Ok(usize::try_from(count.max(0)).unwrap_or(usize::MAX))
}
