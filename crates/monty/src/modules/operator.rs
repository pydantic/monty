//! Implementation of Python's `operator` module.
//!
//! The function forms run as frozen Python functions (see [`crate::frozen`]) that
//! apply the operator syntax itself, so results, errors and user dunder methods
//! (which may suspend) are exactly what `a + b` would give. `itemgetter`,
//! `attrgetter` and `methodcaller` are in [`crate::types::operator_callable`].
//! See `limitations/operator.md` for what is left out. Unimplemented names are
//! absent from the namespace, so they raise `AttributeError` up front.

use crate::{
    args::ArgValues,
    builtins::{Builtins, BuiltinsFunctions},
    bytecode::{CallResult, VM},
    exception_private::{ExcType, ExcTypeExt, RunResult},
    frozen::FrozenFunction,
    heap::{DropWithContext, HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{BuiltinType, Module},
    value::Value,
};

/// Applies a two-operand operator chosen by an integer code (see [`BinaryOp`]).
pub(crate) const BINARY_SOURCE: &str = r"
def __monty_operator_binary(op, a, b):
    if op == 0:
        return a + b
    if op == 1:
        return a - b
    if op == 2:
        return a * b
    if op == 3:
        return a / b
    if op == 4:
        return a // b
    if op == 5:
        return a % b
    if op == 6:
        return a ** b
    if op == 7:
        return a == b
    if op == 8:
        return a != b
    if op == 9:
        return a < b
    if op == 10:
        return a <= b
    if op == 11:
        return a > b
    if op == 12:
        return a >= b
    if op == 13:
        return a is b
    if op == 14:
        return a is not b
    if op == 15:
        return b in a
    if op == 16:
        return a[b]
    if op == 17:
        return a & b
    if op == 18:
        return a | b
    if op == 19:
        return a ^ b
    if op == 20:
        return a << b
    if op == 21:
        return a >> b
    return a @ b
";

/// Applies a one-operand operator chosen by an integer code.
pub(crate) const UNARY_SOURCE: &str = r"
def __monty_operator_unary(op, a):
    if op == 0:
        return -a
    if op == 1:
        return +a
    if op == 2:
        return ~a
    if op == 3:
        return not a
    return True if a else False
";

/// Applies an augmented assignment chosen by an integer code and returns the target.
pub(crate) const INPLACE_SOURCE: &str = r"
def __monty_operator_inplace(op, a, b):
    if op == 0:
        a += b
    elif op == 1:
        a -= b
    elif op == 2:
        a *= b
    elif op == 3:
        a /= b
    elif op == 4:
        a //= b
    elif op == 5:
        a %= b
    elif op == 6:
        a **= b
    elif op == 7:
        a &= b
    elif op == 8:
        a |= b
    elif op == 9:
        a ^= b
    elif op == 10:
        a <<= b
    else:
        a >>= b
    return a
";

/// `a[b] = c`.
pub(crate) const SETITEM_SOURCE: &str = r"
def __monty_operator_setitem(a, b, c):
    a[b] = c
";

/// Counts the items identical or equal to `b`.
pub(crate) const COUNT_OF_SOURCE: &str = r"
def __monty_operator_count_of(a, b):
    count = 0
    for item in a:
        if item is b or item == b:
            count += 1
    return count
";

/// Finds the first item identical or equal to `b`.
pub(crate) const INDEX_OF_SOURCE: &str = r"
def __monty_operator_index_of(a, b, value_error):
    index = 0
    for item in a:
        if item is b or item == b:
            return index
        index += 1
    raise value_error('sequence.index(x): x not in sequence')
";

/// Codes of the two-operand operators, matching [`BINARY_SOURCE`].
#[derive(Debug, Clone, Copy)]
pub(crate) enum BinaryOp {
    Add = 0,
    Sub,
    Mul,
    Truediv,
    Floordiv,
    Mod,
    Pow,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Is,
    IsNot,
    Contains,
    Getitem,
    And,
    Or,
    Xor,
    Lshift,
    Rshift,
    Matmul,
}

/// `operator` module functions, each a Python-visible callable.
///
/// `itemgetter`, `attrgetter` and `methodcaller` are absent because they are
/// exposed as type objects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum OperatorFunctions {
    Add,
    Sub,
    Mul,
    Truediv,
    Floordiv,
    Mod,
    Pow,
    Neg,
    Pos,
    Abs,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    #[strum(serialize = "not_")]
    NotUnderscore,
    Truth,
    #[strum(serialize = "is_")]
    IsUnderscore,
    IsNot,
    Contains,
    Getitem,
    Concat,
    #[strum(serialize = "countOf")]
    Countof,
    #[strum(serialize = "indexOf")]
    Indexof,
    #[strum(serialize = "and_")]
    AndUnderscore,
    #[strum(serialize = "or_")]
    OrUnderscore,
    Xor,
    Invert,
    Lshift,
    Rshift,
    Index,
    Matmul,
    Iadd,
    Isub,
    Imul,
    Itruediv,
    Ifloordiv,
    Imod,
    Ipow,
    Iand,
    Ior,
    Ixor,
    Ilshift,
    Irshift,
    Iconcat,
    Setitem,
    Call,
}

/// How a function is carried out.
enum Plan {
    Binary(BinaryOp),
    /// A unary operator by its code in [`UNARY_SOURCE`].
    Unary(i64),
    /// An in-place operator by its code in [`INPLACE_SOURCE`].
    Inplace(i64),
    Frozen(FrozenFunction),
    Abs,
    Index,
    Call,
}

impl OperatorFunctions {
    /// Every function with the name it is bound to in the module.
    const ALL: [(StaticStrings, Self); 48] = [
        (StaticStrings::Add, Self::Add),
        (StaticStrings::Sub, Self::Sub),
        (StaticStrings::Mul, Self::Mul),
        (StaticStrings::Truediv, Self::Truediv),
        (StaticStrings::Floordiv, Self::Floordiv),
        (StaticStrings::Mod, Self::Mod),
        (StaticStrings::Pow, Self::Pow),
        (StaticStrings::Neg, Self::Neg),
        (StaticStrings::Pos, Self::Pos),
        (StaticStrings::Abs, Self::Abs),
        (StaticStrings::Eq, Self::Eq),
        (StaticStrings::Ne, Self::Ne),
        (StaticStrings::Lt, Self::Lt),
        (StaticStrings::Le, Self::Le),
        (StaticStrings::Gt, Self::Gt),
        (StaticStrings::Ge, Self::Ge),
        (StaticStrings::NotUnderscore, Self::NotUnderscore),
        (StaticStrings::Truth, Self::Truth),
        (StaticStrings::IsUnderscore, Self::IsUnderscore),
        (StaticStrings::IsNot, Self::IsNot),
        (StaticStrings::Contains, Self::Contains),
        (StaticStrings::Getitem, Self::Getitem),
        (StaticStrings::Concat, Self::Concat),
        (StaticStrings::Countof, Self::Countof),
        (StaticStrings::Indexof, Self::Indexof),
        (StaticStrings::AndUnderscore, Self::AndUnderscore),
        (StaticStrings::OrUnderscore, Self::OrUnderscore),
        (StaticStrings::Xor, Self::Xor),
        (StaticStrings::Invert, Self::Invert),
        (StaticStrings::Lshift, Self::Lshift),
        (StaticStrings::Rshift, Self::Rshift),
        (StaticStrings::Index, Self::Index),
        (StaticStrings::Matmul, Self::Matmul),
        (StaticStrings::Iadd, Self::Iadd),
        (StaticStrings::Isub, Self::Isub),
        (StaticStrings::Imul, Self::Imul),
        (StaticStrings::Itruediv, Self::Itruediv),
        (StaticStrings::Ifloordiv, Self::Ifloordiv),
        (StaticStrings::Imod, Self::Imod),
        (StaticStrings::Ipow, Self::Ipow),
        (StaticStrings::Iand, Self::Iand),
        (StaticStrings::Ior, Self::Ior),
        (StaticStrings::Ixor, Self::Ixor),
        (StaticStrings::Ilshift, Self::Ilshift),
        (StaticStrings::Irshift, Self::Irshift),
        (StaticStrings::Iconcat, Self::Iconcat),
        (StaticStrings::Setitem, Self::Setitem),
        (StaticStrings::Call, Self::Call),
    ];

    /// How this function is carried out.
    fn plan(self) -> Plan {
        match self {
            Self::Add | Self::Concat => Plan::Binary(BinaryOp::Add),
            Self::Sub => Plan::Binary(BinaryOp::Sub),
            Self::Mul => Plan::Binary(BinaryOp::Mul),
            Self::Truediv => Plan::Binary(BinaryOp::Truediv),
            Self::Floordiv => Plan::Binary(BinaryOp::Floordiv),
            Self::Mod => Plan::Binary(BinaryOp::Mod),
            Self::Pow => Plan::Binary(BinaryOp::Pow),
            Self::Eq => Plan::Binary(BinaryOp::Eq),
            Self::Ne => Plan::Binary(BinaryOp::Ne),
            Self::Lt => Plan::Binary(BinaryOp::Lt),
            Self::Le => Plan::Binary(BinaryOp::Le),
            Self::Gt => Plan::Binary(BinaryOp::Gt),
            Self::Ge => Plan::Binary(BinaryOp::Ge),
            Self::IsUnderscore => Plan::Binary(BinaryOp::Is),
            Self::IsNot => Plan::Binary(BinaryOp::IsNot),
            Self::Contains => Plan::Binary(BinaryOp::Contains),
            Self::Getitem => Plan::Binary(BinaryOp::Getitem),
            Self::AndUnderscore => Plan::Binary(BinaryOp::And),
            Self::OrUnderscore => Plan::Binary(BinaryOp::Or),
            Self::Xor => Plan::Binary(BinaryOp::Xor),
            Self::Lshift => Plan::Binary(BinaryOp::Lshift),
            Self::Rshift => Plan::Binary(BinaryOp::Rshift),
            Self::Matmul => Plan::Binary(BinaryOp::Matmul),
            Self::Neg => Plan::Unary(0),
            Self::Pos => Plan::Unary(1),
            Self::Invert => Plan::Unary(2),
            Self::NotUnderscore => Plan::Unary(3),
            Self::Truth => Plan::Unary(4),
            Self::Iadd | Self::Iconcat => Plan::Inplace(0),
            Self::Isub => Plan::Inplace(1),
            Self::Imul => Plan::Inplace(2),
            Self::Itruediv => Plan::Inplace(3),
            Self::Ifloordiv => Plan::Inplace(4),
            Self::Imod => Plan::Inplace(5),
            Self::Ipow => Plan::Inplace(6),
            Self::Iand => Plan::Inplace(7),
            Self::Ior => Plan::Inplace(8),
            Self::Ixor => Plan::Inplace(9),
            Self::Ilshift => Plan::Inplace(10),
            Self::Irshift => Plan::Inplace(11),
            Self::Setitem => Plan::Frozen(FrozenFunction::OperatorSetitem),
            Self::Countof => Plan::Frozen(FrozenFunction::OperatorCountOf),
            Self::Indexof => Plan::Frozen(FrozenFunction::OperatorIndexOf),
            Self::Abs => Plan::Abs,
            Self::Index => Plan::Index,
            Self::Call => Plan::Call,
        }
    }
}

/// Creates the `operator` module on the heap.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::Operator, vm.interns);
    for (name, function) in OperatorFunctions::ALL {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Operator(function)), vm);
    }
    for (name, builtin) in [
        (StaticStrings::Itemgetter, BuiltinType::ItemGetter),
        (StaticStrings::Attrgetter, BuiltinType::AttrGetter),
        (StaticStrings::Methodcaller, BuiltinType::MethodCaller),
    ] {
        module.set_attr(name, Value::Builtin(Builtins::Type(builtin)), vm);
    }
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// Dispatches a call to an `operator` module function.
///
/// CPython's C functions are positional-only, so keywords are refused first,
/// except for `call`, which forwards them.
pub(super) fn call(vm: &mut VM<'_>, function: OperatorFunctions, args: ArgValues) -> RunResult<CallResult> {
    let name = function.to_string();
    let c_name = format!("_operator.{name}");
    let plan = function.plan();
    if let Plan::Call = plan {
        return call_call(vm, args);
    }
    let args = args.reject_kwargs(&c_name, vm.heap)?;
    match plan {
        Plan::Binary(op) => {
            let (a, b) = args.get_two_args(&name, vm.heap)?;
            vm.call_frozen_exact(FrozenFunction::OperatorBinary, [Value::Int(op as i64), a, b])
        }
        Plan::Unary(code) => {
            let a = args.get_one_arg(&c_name, vm.heap)?;
            vm.call_frozen_exact(FrozenFunction::OperatorUnary, [Value::Int(code), a])
        }
        Plan::Inplace(code) => {
            let (a, b) = args.get_two_args(&name, vm.heap)?;
            vm.call_frozen_exact(FrozenFunction::OperatorInplace, [Value::Int(code), a, b])
        }
        Plan::Frozen(FrozenFunction::OperatorSetitem) => {
            let (a, b, c) = take_three(args, &name, vm)?;
            vm.call_frozen_exact(FrozenFunction::OperatorSetitem, [a, b, c])
        }
        Plan::Frozen(FrozenFunction::OperatorIndexOf) => {
            let (a, b) = args.get_two_args(&name, vm.heap)?;
            vm.call_frozen_exact(
                FrozenFunction::OperatorIndexOf,
                [a, b, Value::Builtin(Builtins::ExcType(ExcType::ValueError))],
            )
        }
        Plan::Frozen(frozen) => {
            let (a, b) = args.get_two_args(&name, vm.heap)?;
            vm.call_frozen_exact(frozen, [a, b])
        }
        Plan::Abs => {
            let a = args.get_one_arg(&c_name, vm.heap)?;
            vm.call_function(
                &Value::Builtin(Builtins::Function(BuiltinsFunctions::Abs)),
                ArgValues::One(a),
            )
        }
        Plan::Index => {
            let a = args.get_one_arg(&c_name, vm.heap)?;
            let result = a.py_index(vm);
            a.drop_with(vm);
            result.map(CallResult::Value)
        }
        Plan::Call => unreachable!("handled above"),
    }
}

/// Collects exactly three positional arguments for `setitem`.
fn take_three(args: ArgValues, name: &str, vm: &mut VM<'_>) -> RunResult<(Value, Value, Value)> {
    let (positional, keywords) = args.into_parts();
    keywords.drop_with(vm);
    let mut values: Vec<Value> = positional.collect();
    if values.len() != 3 {
        let count = values.len();
        values.drop_with(vm);
        return Err(ExcType::type_error_arg_count(name, 3, count));
    }
    let c = values.pop().expect("length checked");
    let b = values.pop().expect("length checked");
    let a = values.pop().expect("length checked");
    Ok((a, b, c))
}

/// `operator.call(obj, /, *args, **kwargs)`.
fn call_call(vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
    let (positional, keywords) = args.into_parts();
    let mut values: Vec<Value> = positional.collect();
    if values.is_empty() {
        keywords.drop_with(vm);
        return Err(ExcType::type_error_at_least("call", 1, 0));
    }
    let target = values.remove(0);
    let result = vm.call_function(&target, ArgValues::from_parts(values, keywords));
    target.drop_with(vm);
    result
}
