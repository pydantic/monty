//! Native exception class definitions sharing the exception runtime representation.

use crate::{
    exception_private::ExcType,
    types::{Type, class_def::ClassDef},
};

macro_rules! builtin_exceptions {
    ($($name:ident => $base:ident),+ $(,)?) => {
        $(
            pub(crate) struct $name;

            impl ClassDef for $name {
                const NAME: &'static str = stringify!($name);
                const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
                const BASES: &'static [Type] = &[Type::$base];
                const EXC_TYPE: Option<ExcType> = Some(ExcType::$name);
            }
        )+
    };
}

builtin_exceptions! {
    Exception => BaseException,
    BaseException => Object,
    SystemExit => BaseException,
    KeyboardInterrupt => BaseException,
    ArithmeticError => Exception,
    OverflowError => ArithmeticError,
    ZeroDivisionError => ArithmeticError,
    LookupError => Exception,
    IndexError => LookupError,
    KeyError => LookupError,
    RuntimeError => Exception,
    NotImplementedError => RuntimeError,
    RecursionError => RuntimeError,
    AttributeError => Exception,
    NameError => Exception,
    UnboundLocalError => NameError,
    ValueError => Exception,
    UnicodeDecodeError => ValueError,
    UnicodeEncodeError => ValueError,
    ImportError => Exception,
    ModuleNotFoundError => ImportError,
    OSError => Exception,
    FileNotFoundError => OSError,
    FileExistsError => OSError,
    IsADirectoryError => OSError,
    NotADirectoryError => OSError,
    PermissionError => OSError,
    TimeoutError => OSError,
    BlockingIOError => OSError,
    AssertionError => Exception,
    MemoryError => Exception,
    StopIteration => Exception,
    SyntaxError => Exception,
    TypeError => Exception,
}

/// Defines `FrozenInstanceError` over shared exception storage.
pub(crate) struct FrozenInstanceError;

impl ClassDef for FrozenInstanceError {
    const NAME: &'static str = "FrozenInstanceError";
    const BASES: &'static [Type] = &[Type::AttributeError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::FrozenInstanceError);
}

/// Defines `json.JSONDecodeError` over shared exception storage.
pub(crate) struct JsonDecodeError;

impl ClassDef for JsonDecodeError {
    const NAME: &'static str = "json.JSONDecodeError";
    const BASES: &'static [Type] = &[Type::ValueError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::JsonDecodeError);
}

/// Defines `tomllib.TOMLDecodeError` over shared exception storage.
pub(crate) struct TomlDecodeError;

impl ClassDef for TomlDecodeError {
    const NAME: &'static str = "tomllib.TOMLDecodeError";
    const BASES: &'static [Type] = &[Type::ValueError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::TomlDecodeError);
}

/// Defines `io.UnsupportedOperation` over shared exception storage.
pub(crate) struct UnsupportedOperation;

impl ClassDef for UnsupportedOperation {
    const NAME: &'static str = "io.UnsupportedOperation";
    const BASES: &'static [Type] = &[Type::OSError, Type::ValueError];
    const MRO: Option<&'static [Type]> = Some(&[
        Type::UnsupportedOperation,
        Type::OSError,
        Type::ValueError,
        Type::Exception,
        Type::BaseException,
        Type::Object,
    ]);
    const EXC_TYPE: Option<ExcType> = Some(ExcType::UnsupportedOperation);
}

/// Defines `re.PatternError` over shared exception storage.
pub(crate) struct RePatternError;

impl ClassDef for RePatternError {
    const NAME: &'static str = "re.PatternError";
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::RePatternError);
}

/// Defines `binascii.Error` over shared exception storage.
pub(crate) struct BinasciiError;

impl ClassDef for BinasciiError {
    const NAME: &'static str = "binascii.Error";
    const BASES: &'static [Type] = &[Type::ValueError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::BinasciiError);
}

/// Defines `binascii.Incomplete` over shared exception storage.
pub(crate) struct BinasciiIncomplete;

impl ClassDef for BinasciiIncomplete {
    const NAME: &'static str = "binascii.Incomplete";
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::BinasciiIncomplete);
}

/// Defines `_csv.Error` over shared exception storage.
pub(crate) struct CsvError;

impl ClassDef for CsvError {
    const NAME: &'static str = "_csv.Error";
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::CsvError);
}

/// Defines `decimal.DecimalException` over shared exception storage.
pub(crate) struct DecimalException;

impl ClassDef for DecimalException {
    const NAME: &'static str = "decimal.DecimalException";
    const BASES: &'static [Type] = &[Type::ArithmeticError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::DecimalException);
}

/// Defines `decimal.Clamped` over shared exception storage.
pub(crate) struct DecimalClamped;

impl ClassDef for DecimalClamped {
    const NAME: &'static str = "decimal.Clamped";
    const BASES: &'static [Type] = &[Type::DecimalException];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::DecimalClamped);
}

/// Defines `decimal.InvalidOperation` over shared exception storage.
pub(crate) struct DecimalInvalidOperation;

impl ClassDef for DecimalInvalidOperation {
    const NAME: &'static str = "decimal.InvalidOperation";
    const BASES: &'static [Type] = &[Type::DecimalException];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::DecimalInvalidOperation);
}

/// Defines `decimal.ConversionSyntax` over shared exception storage.
pub(crate) struct DecimalConversionSyntax;

impl ClassDef for DecimalConversionSyntax {
    const NAME: &'static str = "decimal.ConversionSyntax";
    const BASES: &'static [Type] = &[Type::DecimalInvalidOperation];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::DecimalConversionSyntax);
}

/// Defines `decimal.DivisionByZero` over shared exception storage.
pub(crate) struct DecimalDivisionByZero;

impl ClassDef for DecimalDivisionByZero {
    const NAME: &'static str = "decimal.DivisionByZero";
    const BASES: &'static [Type] = &[Type::DecimalException, Type::ZeroDivisionError];
    const MRO: Option<&'static [Type]> = Some(&[
        Type::DecimalDivisionByZero,
        Type::DecimalException,
        Type::ZeroDivisionError,
        Type::ArithmeticError,
        Type::Exception,
        Type::BaseException,
        Type::Object,
    ]);
    const EXC_TYPE: Option<ExcType> = Some(ExcType::DecimalDivisionByZero);
}

/// Defines `decimal.DivisionImpossible` over shared exception storage.
pub(crate) struct DecimalDivisionImpossible;

impl ClassDef for DecimalDivisionImpossible {
    const NAME: &'static str = "decimal.DivisionImpossible";
    const BASES: &'static [Type] = &[Type::DecimalInvalidOperation];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::DecimalDivisionImpossible);
}

/// Defines `decimal.DivisionUndefined` over shared exception storage.
pub(crate) struct DecimalDivisionUndefined;

impl ClassDef for DecimalDivisionUndefined {
    const NAME: &'static str = "decimal.DivisionUndefined";
    const BASES: &'static [Type] = &[Type::DecimalInvalidOperation, Type::ZeroDivisionError];
    const MRO: Option<&'static [Type]> = Some(&[
        Type::DecimalDivisionUndefined,
        Type::DecimalInvalidOperation,
        Type::DecimalException,
        Type::ZeroDivisionError,
        Type::ArithmeticError,
        Type::Exception,
        Type::BaseException,
        Type::Object,
    ]);
    const EXC_TYPE: Option<ExcType> = Some(ExcType::DecimalDivisionUndefined);
}

/// Defines `decimal.Inexact` over shared exception storage.
pub(crate) struct DecimalInexact;

impl ClassDef for DecimalInexact {
    const NAME: &'static str = "decimal.Inexact";
    const BASES: &'static [Type] = &[Type::DecimalException];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::DecimalInexact);
}

/// Defines `decimal.InvalidContext` over shared exception storage.
pub(crate) struct DecimalInvalidContext;

impl ClassDef for DecimalInvalidContext {
    const NAME: &'static str = "decimal.InvalidContext";
    const BASES: &'static [Type] = &[Type::DecimalInvalidOperation];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::DecimalInvalidContext);
}

/// Defines `decimal.Rounded` over shared exception storage.
pub(crate) struct DecimalRounded;

impl ClassDef for DecimalRounded {
    const NAME: &'static str = "decimal.Rounded";
    const BASES: &'static [Type] = &[Type::DecimalException];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::DecimalRounded);
}

/// Defines `decimal.Subnormal` over shared exception storage.
pub(crate) struct DecimalSubnormal;

impl ClassDef for DecimalSubnormal {
    const NAME: &'static str = "decimal.Subnormal";
    const BASES: &'static [Type] = &[Type::DecimalException];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::DecimalSubnormal);
}

/// Defines `decimal.Overflow` over shared exception storage.
pub(crate) struct DecimalOverflow;

impl ClassDef for DecimalOverflow {
    const NAME: &'static str = "decimal.Overflow";
    const BASES: &'static [Type] = &[Type::DecimalInexact, Type::DecimalRounded];
    const MRO: Option<&'static [Type]> = Some(&[
        Type::DecimalOverflow,
        Type::DecimalInexact,
        Type::DecimalRounded,
        Type::DecimalException,
        Type::ArithmeticError,
        Type::Exception,
        Type::BaseException,
        Type::Object,
    ]);
    const EXC_TYPE: Option<ExcType> = Some(ExcType::DecimalOverflow);
}

/// Defines `decimal.Underflow` over shared exception storage.
pub(crate) struct DecimalUnderflow;

impl ClassDef for DecimalUnderflow {
    const NAME: &'static str = "decimal.Underflow";
    const BASES: &'static [Type] = &[Type::DecimalInexact, Type::DecimalRounded, Type::DecimalSubnormal];
    const MRO: Option<&'static [Type]> = Some(&[
        Type::DecimalUnderflow,
        Type::DecimalInexact,
        Type::DecimalRounded,
        Type::DecimalSubnormal,
        Type::DecimalException,
        Type::ArithmeticError,
        Type::Exception,
        Type::BaseException,
        Type::Object,
    ]);
    const EXC_TYPE: Option<ExcType> = Some(ExcType::DecimalUnderflow);
}

/// Defines `decimal.FloatOperation` over shared exception storage.
pub(crate) struct DecimalFloatOperation;

impl ClassDef for DecimalFloatOperation {
    const NAME: &'static str = "decimal.FloatOperation";
    const BASES: &'static [Type] = &[Type::DecimalException, Type::TypeError];
    const MRO: Option<&'static [Type]> = Some(&[
        Type::DecimalFloatOperation,
        Type::DecimalException,
        Type::ArithmeticError,
        Type::TypeError,
        Type::Exception,
        Type::BaseException,
        Type::Object,
    ]);
    const EXC_TYPE: Option<ExcType> = Some(ExcType::DecimalFloatOperation);
}

/// Defines `statistics.StatisticsError` over shared exception storage.
pub(crate) struct StatisticsError;

impl ClassDef for StatisticsError {
    const NAME: &'static str = "statistics.StatisticsError";
    const BASES: &'static [Type] = &[Type::ValueError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::StatisticsError);
}

pub(crate) struct InvalidStateError;

impl ClassDef for InvalidStateError {
    const NAME: &'static str = "asyncio.exceptions.InvalidStateError";
    const EXC_TYPE: Option<ExcType> = Some(ExcType::InvalidStateError);
    const BASES: &'static [Type] = &[Type::Exception];
}
