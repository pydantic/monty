//! Implementation of the `decimal` module.
//!
//! Exposes the `Decimal` type, the full exception taxonomy, and the
//! rounding-mode string constants. Arithmetic, comparison and method behaviour
//! lives on the runtime [`Decimal`] type. Monty has no mutable `Context`:
//! arithmetic always runs under CPython's default context (`prec=28`,
//! `ROUND_HALF_EVEN`); methods that accept a per-call `rounding=` argument in
//! CPython accept it here too (see `limitations/decimal.md`).
//!
//! [`Decimal`]: crate::types::decimal::Decimal

use crate::{
    builtins::Builtins,
    bytecode::VM,
    exception_private::ExcType,
    heap::{HeapData, HeapId},
    intern::StaticStrings,
    resource::{ResourceError, ResourceTracker},
    types::{Module, Type, decimal::ROUNDING_MODES},
    value::Value,
};

/// Creates the `decimal` module and allocates it on the heap.
///
/// # Panics
///
/// Panics if the required strings have not been pre-interned during prepare phase.
pub fn create_module(vm: &mut VM<'_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
    let mut module = Module::new(StaticStrings::Decimal);

    // The Decimal type itself.
    module.set_attr(
        StaticStrings::DecimalClass,
        Value::Builtin(Builtins::Type(Type::Decimal)),
        vm,
    );

    // The full CPython exception taxonomy. `(name, ExcType)` pairs registered as
    // module attributes; the subclass relationships (e.g. `Underflow ⊂ (Inexact,
    // Rounded, Subnormal)`) live in `ExcType::is_subclass_of`.
    for (name, exc) in [
        (StaticStrings::DecimalExceptionName, ExcType::DecimalException),
        (StaticStrings::InvalidOperation, ExcType::DecimalInvalidOperation),
        (StaticStrings::DivisionByZero, ExcType::DecimalDivisionByZero),
        (StaticStrings::DecimalOverflowName, ExcType::DecimalOverflow),
        (StaticStrings::DecimalInexactName, ExcType::DecimalInexact),
        (StaticStrings::DecimalRoundedName, ExcType::DecimalRounded),
        (StaticStrings::DecimalSubnormalName, ExcType::DecimalSubnormal),
        (StaticStrings::DecimalClampedName, ExcType::DecimalClamped),
        (StaticStrings::DecimalUnderflowName, ExcType::DecimalUnderflow),
        (StaticStrings::DecimalFloatOperationName, ExcType::DecimalFloatOperation),
        // The finer InvalidOperation condition subtypes. Importable/catchable so
        // `except decimal.ConversionSyntax:` matches CPython; Monty raises
        // `InvalidOperation` (with the condition in its message), never these.
        (StaticStrings::ConversionSyntax, ExcType::DecimalConversionSyntax),
        (StaticStrings::DivisionImpossible, ExcType::DecimalDivisionImpossible),
        (StaticStrings::DivisionUndefined, ExcType::DecimalDivisionUndefined),
        (StaticStrings::InvalidContext, ExcType::DecimalInvalidContext),
    ] {
        module.set_attr(name, Value::Builtin(Builtins::ExcType(exc)), vm);
    }

    // Rounding-mode string constants.
    for (rounding_mode, _) in ROUNDING_MODES {
        module.set_attr(rounding_mode, Value::from(rounding_mode), vm);
    }

    vm.heap.allocate(HeapData::Module(module))
}
