//! The `decimal` module: `Decimal`, `Context`, the signals and the context functions.
//!
//! The types live in `types::decimal`; this module only assembles the namespace.
//! The three template contexts are session state, so every `import decimal`
//! exposes the same `DefaultContext` object (see `py_context.rs`).

use crate::{
    args::ArgValues,
    builtins::Builtins,
    bytecode::VM,
    exception_private::{ExcType, RunResult},
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    modules::ModuleFunctions,
    types::{
        BuiltinType, Module, NamedTupleClass, allocate_string,
        decimal::{
            context::{MAX_EMAX, MAX_PREC, MIN_EMIN, MIN_ETINY, Rounding},
            py_context::{self, IEEE_CONTEXT_MAX_BITS, Template},
        },
    },
    value::{EitherStr, Value},
};

/// The functions of the `decimal` module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, serde::Serialize, serde::Deserialize)]
pub(crate) enum DecimalFunctions {
    #[strum(serialize = "getcontext")]
    Getcontext,
    #[strum(serialize = "setcontext")]
    Setcontext,
    #[strum(serialize = "localcontext")]
    Localcontext,
    #[strum(serialize = "IEEEContext")]
    IeeeContext,
}

/// The signal and condition classes, as `(attribute, exception type)`.
const EXCEPTIONS: [(StaticStrings, ExcType); 14] = [
    (StaticStrings::DecimalException, ExcType::DecimalException),
    (StaticStrings::Clamped, ExcType::DecimalClamped),
    (StaticStrings::InvalidOperation, ExcType::DecimalInvalidOperation),
    (StaticStrings::ConversionSyntax, ExcType::DecimalConversionSyntax),
    (StaticStrings::DivisionByZero, ExcType::DecimalDivisionByZero),
    (StaticStrings::DivisionImpossible, ExcType::DecimalDivisionImpossible),
    (StaticStrings::DivisionUndefined, ExcType::DecimalDivisionUndefined),
    (StaticStrings::Inexact, ExcType::DecimalInexact),
    (StaticStrings::InvalidContext, ExcType::DecimalInvalidContext),
    (StaticStrings::Rounded, ExcType::DecimalRounded),
    (StaticStrings::Subnormal, ExcType::DecimalSubnormal),
    (StaticStrings::Overflow, ExcType::DecimalOverflow),
    (StaticStrings::Underflow, ExcType::DecimalUnderflow),
    (StaticStrings::FloatOperation, ExcType::DecimalFloatOperation),
];

/// The rounding-mode constants, in [`Rounding::ALL`] order.
const ROUNDING_NAMES: [StaticStrings; 8] = [
    StaticStrings::RoundCeiling,
    StaticStrings::RoundFloor,
    StaticStrings::RoundUp,
    StaticStrings::RoundDown,
    StaticStrings::RoundHalfUp,
    StaticStrings::RoundHalfDown,
    StaticStrings::RoundHalfEven,
    StaticStrings::Round05up,
];

/// Creates the `decimal` module.
pub fn create_module(vm: &mut VM<'_>) -> HeapId {
    let mut module = Module::new(StaticStrings::DecimalMod, vm.interns);
    module.set_attr(
        StaticStrings::DecimalType,
        Value::Builtin(Builtins::Type(BuiltinType::Decimal)),
        vm,
    );
    module.set_attr(
        StaticStrings::ContextType,
        Value::Builtin(Builtins::Type(BuiltinType::DecimalContext)),
        vm,
    );
    for (name, function) in [
        (StaticStrings::Getcontext, DecimalFunctions::Getcontext),
        (StaticStrings::Setcontext, DecimalFunctions::Setcontext),
        (StaticStrings::Localcontext, DecimalFunctions::Localcontext),
        (StaticStrings::IeeeContext, DecimalFunctions::IeeeContext),
    ] {
        module.set_attr(name, Value::ModuleFunction(ModuleFunctions::Decimal(function)), vm);
    }
    for (name, exc_type) in EXCEPTIONS {
        module.set_attr(name, Value::Builtin(Builtins::ExcType(exc_type)), vm);
    }
    for (name, rounding) in ROUNDING_NAMES.into_iter().zip(Rounding::ALL) {
        module.set_attr(name, allocate_string(rounding.name(), vm.heap), vm);
    }
    for (name, template) in [
        (StaticStrings::DefaultContext, Template::Default),
        (StaticStrings::BasicContext, Template::Basic),
        (StaticStrings::ExtendedContext, Template::Extended),
    ] {
        let id = py_context::template_id(vm, template);
        vm.heap.inc_ref(id);
        module.set_attr(name, Value::Ref(id), vm);
    }
    for (name, value) in [
        (StaticStrings::MaxPrec, MAX_PREC),
        (StaticStrings::MaxEmax, MAX_EMAX),
        (StaticStrings::MinEmin, MIN_EMIN),
        (StaticStrings::MinEtiny, MIN_ETINY),
        (StaticStrings::IeeeContextMaxBits, IEEE_CONTEXT_MAX_BITS),
    ] {
        module.set_attr(name, Value::Int(value), vm);
    }
    module.set_attr(StaticStrings::HaveThreads, Value::Bool(true), vm);
    module.set_attr(StaticStrings::HaveContextvar, Value::Bool(true), vm);
    module.set_attr(StaticStrings::DunderVersion, allocate_string("1.70", vm.heap), vm);
    module.set_attr(
        StaticStrings::DunderLibmpdecVersion,
        allocate_string("4.0.1", vm.heap),
        vm,
    );
    let decimal_tuple = decimal_tuple_class(vm);
    module.set_attr(StaticStrings::DecimalTuple, decimal_tuple, vm);
    vm.heap.allocate(HeapData::Module(Box::new(module)))
}

/// `decimal.DecimalTuple`, the named tuple class `Decimal.as_tuple()` builds.
fn decimal_tuple_class(vm: &mut VM<'_>) -> Value {
    let field = |name| EitherStr::Interned(vm.interns.intern_static(name));
    let fields = vec![
        field(StaticStrings::Sign),
        field(StaticStrings::Digits),
        field(StaticStrings::Exponent),
    ];
    let name = field(StaticStrings::DecimalTuple);
    let module = Value::InternString(vm.interns.intern_static(StaticStrings::DecimalMod));
    let class = NamedTupleClass::new(name, fields, Vec::new(), module);
    Value::Ref(vm.heap.allocate(HeapData::NamedTupleClass(Box::new(class))))
}

/// Dispatches a `decimal` module function.
pub(super) fn call(vm: &mut VM<'_>, function: DecimalFunctions, args: ArgValues) -> RunResult<Value> {
    match function {
        DecimalFunctions::Getcontext => py_context::getcontext(vm, args),
        DecimalFunctions::Setcontext => py_context::setcontext(vm, args),
        DecimalFunctions::Localcontext => py_context::localcontext(vm, args),
        DecimalFunctions::IeeeContext => py_context::ieee_context(vm, args),
    }
}
