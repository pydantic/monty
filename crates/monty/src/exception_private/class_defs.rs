//! Native exception class definitions sharing the exception runtime representation.

use crate::{
    exception_private::ExcType,
    types::{Type, class_def::ClassDef},
};

/// Defines `Exception` over shared exception storage.
pub(crate) struct Exception;

impl ClassDef for Exception {
    const NAME: &'static str = "Exception";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::BaseException];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::Exception);
}

/// Defines `BaseException` over shared exception storage.
pub(crate) struct BaseException;

impl ClassDef for BaseException {
    const NAME: &'static str = "BaseException";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::Object];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::BaseException);
}

/// Defines `SystemExit` over shared exception storage.
pub(crate) struct SystemExit;

impl ClassDef for SystemExit {
    const NAME: &'static str = "SystemExit";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::BaseException];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::SystemExit);
}

/// Defines `KeyboardInterrupt` over shared exception storage.
pub(crate) struct KeyboardInterrupt;

impl ClassDef for KeyboardInterrupt {
    const NAME: &'static str = "KeyboardInterrupt";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::BaseException];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::KeyboardInterrupt);
}

/// Defines `ArithmeticError` over shared exception storage.
pub(crate) struct ArithmeticError;

impl ClassDef for ArithmeticError {
    const NAME: &'static str = "ArithmeticError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::ArithmeticError);
}

/// Defines `OverflowError` over shared exception storage.
pub(crate) struct OverflowError;

impl ClassDef for OverflowError {
    const NAME: &'static str = "OverflowError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::ArithmeticError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::OverflowError);
}

/// Defines `ZeroDivisionError` over shared exception storage.
pub(crate) struct ZeroDivisionError;

impl ClassDef for ZeroDivisionError {
    const NAME: &'static str = "ZeroDivisionError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::ArithmeticError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::ZeroDivisionError);
}

/// Defines `LookupError` over shared exception storage.
pub(crate) struct LookupError;

impl ClassDef for LookupError {
    const NAME: &'static str = "LookupError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::LookupError);
}

/// Defines `IndexError` over shared exception storage.
pub(crate) struct IndexError;

impl ClassDef for IndexError {
    const NAME: &'static str = "IndexError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::LookupError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::IndexError);
}

/// Defines `KeyError` over shared exception storage.
pub(crate) struct KeyError;

impl ClassDef for KeyError {
    const NAME: &'static str = "KeyError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::LookupError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::KeyError);
}

/// Defines `RuntimeError` over shared exception storage.
pub(crate) struct RuntimeError;

impl ClassDef for RuntimeError {
    const NAME: &'static str = "RuntimeError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::RuntimeError);
}

/// Defines `NotImplementedError` over shared exception storage.
pub(crate) struct NotImplementedError;

impl ClassDef for NotImplementedError {
    const NAME: &'static str = "NotImplementedError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::RuntimeError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::NotImplementedError);
}

/// Defines `RecursionError` over shared exception storage.
pub(crate) struct RecursionError;

impl ClassDef for RecursionError {
    const NAME: &'static str = "RecursionError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::RuntimeError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::RecursionError);
}

/// Defines `AttributeError` over shared exception storage.
pub(crate) struct AttributeError;

impl ClassDef for AttributeError {
    const NAME: &'static str = "AttributeError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::AttributeError);
}

/// Defines `FrozenInstanceError` over shared exception storage.
pub(crate) struct FrozenInstanceError;

impl ClassDef for FrozenInstanceError {
    const NAME: &'static str = "FrozenInstanceError";
    const BASES: &'static [Type] = &[Type::AttributeError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::FrozenInstanceError);
}

/// Defines `NameError` over shared exception storage.
pub(crate) struct NameError;

impl ClassDef for NameError {
    const NAME: &'static str = "NameError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::NameError);
}

/// Defines `UnboundLocalError` over shared exception storage.
pub(crate) struct UnboundLocalError;

impl ClassDef for UnboundLocalError {
    const NAME: &'static str = "UnboundLocalError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::NameError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::UnboundLocalError);
}

/// Defines `ValueError` over shared exception storage.
pub(crate) struct ValueError;

impl ClassDef for ValueError {
    const NAME: &'static str = "ValueError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::ValueError);
}

/// Defines `UnicodeDecodeError` over shared exception storage.
pub(crate) struct UnicodeDecodeError;

impl ClassDef for UnicodeDecodeError {
    const NAME: &'static str = "UnicodeDecodeError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::ValueError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::UnicodeDecodeError);
}

/// Defines `UnicodeEncodeError` over shared exception storage.
pub(crate) struct UnicodeEncodeError;

impl ClassDef for UnicodeEncodeError {
    const NAME: &'static str = "UnicodeEncodeError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::ValueError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::UnicodeEncodeError);
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

/// Defines `ImportError` over shared exception storage.
pub(crate) struct ImportError;

impl ClassDef for ImportError {
    const NAME: &'static str = "ImportError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::ImportError);
}

/// Defines `ModuleNotFoundError` over shared exception storage.
pub(crate) struct ModuleNotFoundError;

impl ClassDef for ModuleNotFoundError {
    const NAME: &'static str = "ModuleNotFoundError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::ImportError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::ModuleNotFoundError);
}

/// Defines `OSError` over shared exception storage.
pub(crate) struct OSError;

impl ClassDef for OSError {
    const NAME: &'static str = "OSError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::OSError);
}

/// Defines `FileNotFoundError` over shared exception storage.
pub(crate) struct FileNotFoundError;

impl ClassDef for FileNotFoundError {
    const NAME: &'static str = "FileNotFoundError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::OSError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::FileNotFoundError);
}

/// Defines `FileExistsError` over shared exception storage.
pub(crate) struct FileExistsError;

impl ClassDef for FileExistsError {
    const NAME: &'static str = "FileExistsError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::OSError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::FileExistsError);
}

/// Defines `IsADirectoryError` over shared exception storage.
pub(crate) struct IsADirectoryError;

impl ClassDef for IsADirectoryError {
    const NAME: &'static str = "IsADirectoryError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::OSError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::IsADirectoryError);
}

/// Defines `NotADirectoryError` over shared exception storage.
pub(crate) struct NotADirectoryError;

impl ClassDef for NotADirectoryError {
    const NAME: &'static str = "NotADirectoryError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::OSError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::NotADirectoryError);
}

/// Defines `PermissionError` over shared exception storage.
pub(crate) struct PermissionError;

impl ClassDef for PermissionError {
    const NAME: &'static str = "PermissionError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::OSError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::PermissionError);
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

/// Defines `TimeoutError` over shared exception storage.
pub(crate) struct TimeoutError;

impl ClassDef for TimeoutError {
    const NAME: &'static str = "TimeoutError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::OSError];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::TimeoutError);
}

/// Defines `AssertionError` over shared exception storage.
pub(crate) struct AssertionError;

impl ClassDef for AssertionError {
    const NAME: &'static str = "AssertionError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::AssertionError);
}

/// Defines `MemoryError` over shared exception storage.
pub(crate) struct MemoryError;

impl ClassDef for MemoryError {
    const NAME: &'static str = "MemoryError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::MemoryError);
}

/// Defines `StopIteration` over shared exception storage.
pub(crate) struct StopIteration;

impl ClassDef for StopIteration {
    const NAME: &'static str = "StopIteration";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::StopIteration);
}

/// Defines `SyntaxError` over shared exception storage.
pub(crate) struct SyntaxError;

impl ClassDef for SyntaxError {
    const NAME: &'static str = "SyntaxError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::SyntaxError);
}

/// Defines `TypeError` over shared exception storage.
pub(crate) struct TypeError;

impl ClassDef for TypeError {
    const NAME: &'static str = "TypeError";
    const BUILTIN_NAME: Option<&'static str> = Some(Self::NAME);
    const BASES: &'static [Type] = &[Type::Exception];
    const EXC_TYPE: Option<ExcType> = Some(ExcType::TypeError);
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
