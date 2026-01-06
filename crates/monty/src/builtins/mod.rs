//! Python builtin functions, types, and exception constructors.
//!
//! This module provides the interpreter-native implementation of Python builtins.
//! Each builtin function has its own submodule for organization.

mod abs;
mod all;
mod any;
mod bin;
mod chr;
mod divmod;
mod enumerate;
mod hash;
mod hex;
mod id;
mod isinstance;
mod len;
mod min_max; // min and max share implementation
mod oct;
mod ord;
mod pow;
mod print;
mod repr;
mod reversed;
mod round;
mod sorted;
mod sum;
mod type_;
mod zip;

use std::{fmt::Write, str::FromStr};

use strum::{Display, EnumString, IntoStaticStr};

use crate::{
    args::ArgValues, exception_private::ExcType, heap::Heap, intern::Interns, io::PrintWriter,
    resource::ResourceTracker, run_frame::RunResult, types::Type, value::Value,
};

/// Enumerates every interpreter-native Python builtins
///
/// Uses strum derives for automatic `Display`, `FromStr`, and `AsRef<str>` implementations.
/// All variants serialize to lowercase (e.g., `Print` -> "print").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Builtins {
    /// A builtin function like `print`, `len`, `type`, etc.
    Function(BuiltinsFunctions),
    /// An exception type constructor like `ValueError`, `TypeError`, etc.
    ExcType(ExcType),
    /// A type constructor like `list`, `dict`, `int`, etc.
    Type(Type),
}

impl Builtins {
    /// Calls this builtin with the given arguments.
    ///
    /// # Arguments
    /// * `heap` - The heap for allocating objects
    /// * `args` - The arguments to pass to the callable
    /// * `interns` - String storage for looking up interned names in error messages
    /// * `print` - The print for print output
    pub fn call(
        self,
        heap: &mut Heap<impl ResourceTracker>,
        args: ArgValues,
        interns: &Interns,
        print: &mut impl PrintWriter,
    ) -> RunResult<Value> {
        match self {
            Self::Function(b) => b.call(heap, args, interns, print),
            Self::ExcType(exc) => exc.call(heap, args, interns),
            Self::Type(t) => t.call(heap, args, interns),
        }
    }

    /// Writes the Python repr() string for this callable to a formatter.
    pub fn py_repr_fmt<W: Write>(self, f: &mut W) -> std::fmt::Result {
        match self {
            Self::Function(b) => write!(f, "<built-in function {b}>"),
            Self::ExcType(e) => write!(f, "<class '{e}'>"),
            Self::Type(t) => write!(f, "<class '{t}'>"),
        }
    }

    /// Returns the type of this builtin.
    pub fn py_type(self) -> Type {
        match self {
            Self::Function(_) => Type::BuiltinFunction,
            Self::ExcType(_) => Type::Type,
            Self::Type(_) => Type::Type,
        }
    }

    /// Returns the exception type if this is an ExcType builtin.
    ///
    /// Used for exception type matching in try/except handlers.
    pub fn as_exc_type(self) -> Option<ExcType> {
        match self {
            Self::ExcType(exc_type) => Some(exc_type),
            _ => None,
        }
    }
}

impl FromStr for Builtins {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Priority: BuiltinsFunctions > ExcType > Type
        if let Ok(b) = BuiltinsFunctions::from_str(s) {
            Ok(Self::Function(b))
        } else if let Ok(exc) = ExcType::from_str(s) {
            Ok(Self::ExcType(exc))
        } else if let Ok(t) = Type::from_str(s) {
            Ok(Self::Type(t))
        } else {
            Err(())
        }
    }
}

/// Enumerates every interpreter-native Python builtin function.
///
/// Listed alphabetically per https://docs.python.org/3/library/functions.html
/// Commented-out variants are not yet implemented.
///
/// Note: Type constructors are handled by the `Type` enum, not here.
///
/// Uses strum derives for automatic `Display`, `FromStr`, and `IntoStaticStr` implementations.
/// All variants serialize to lowercase (e.g., `Print` -> "print").
#[derive(
    Debug, Clone, Copy, Display, EnumString, IntoStaticStr, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[strum(serialize_all = "lowercase")]
pub enum BuiltinsFunctions {
    Abs,
    // Aiter,
    All,
    // Anext,
    Any,
    // Ascii,
    Bin,
    // bool - handled by Type enum
    // Breakpoint,
    // bytearray - handled by Type enum
    // bytes - handled by Type enum
    // Callable,
    Chr,
    // Classmethod,
    // Compile,
    // complex - handled by Type enum
    // Delattr,
    // dict - handled by Type enum
    // Dir,
    Divmod,
    Enumerate,
    // Eval,
    // Exec,
    // Filter,
    // float - handled by Type enum
    // Format,
    // frozenset - handled by Type enum
    // Getattr,
    // Globals,
    // Hasattr,
    Hash,
    // Help,
    Hex,
    Id,
    // Input,
    // int - handled by Type enum
    Isinstance,
    // Issubclass,
    // Iter,
    Len,
    // list - handled by Type enum
    // Locals,
    // Map,
    Max,
    // memoryview - handled by Type enum
    Min,
    // Next,
    // object - handled by Type enum
    Oct,
    // Open,
    Ord,
    Pow,
    Print,
    // Property,
    // range - handled by Type enum
    Repr,
    Reversed,
    Round,
    // set - handled by Type enum
    // Setattr,
    // Slice,
    Sorted,
    // Staticmethod,
    // str - handled by Type enum
    Sum,
    // Super,
    // tuple - handled by Type enum
    Type,
    // Vars,
    Zip,
    // __import__ - not planned
}

impl BuiltinsFunctions {
    /// Executes the builtin with the provided positional arguments.
    ///
    /// The `interns` parameter provides access to interned string content for py_str and py_repr.
    /// The `print` parameter is used for print output.
    fn call(
        self,
        heap: &mut Heap<impl ResourceTracker>,
        args: ArgValues,
        interns: &Interns,
        print_writer: &mut impl PrintWriter,
    ) -> RunResult<Value> {
        match self {
            Self::Abs => abs::builtin_abs(heap, args),
            Self::All => all::builtin_all(heap, args, interns),
            Self::Any => any::builtin_any(heap, args, interns),
            Self::Bin => bin::builtin_bin(heap, args),
            Self::Chr => chr::builtin_chr(heap, args),
            Self::Divmod => divmod::builtin_divmod(heap, args),
            Self::Enumerate => enumerate::builtin_enumerate(heap, args, interns),
            Self::Hash => hash::builtin_hash(heap, args, interns),
            Self::Hex => hex::builtin_hex(heap, args),
            Self::Id => id::builtin_id(heap, args),
            Self::Isinstance => isinstance::builtin_isinstance(heap, args),
            Self::Len => len::builtin_len(heap, args, interns),
            Self::Max => min_max::builtin_max(heap, args, interns),
            Self::Min => min_max::builtin_min(heap, args, interns),
            Self::Oct => oct::builtin_oct(heap, args),
            Self::Ord => ord::builtin_ord(heap, args, interns),
            Self::Pow => pow::builtin_pow(heap, args),
            Self::Print => print::builtin_print(heap, args, interns, print_writer),
            Self::Repr => repr::builtin_repr(heap, args, interns),
            Self::Reversed => reversed::builtin_reversed(heap, args, interns),
            Self::Round => round::builtin_round(heap, args),
            Self::Sorted => sorted::builtin_sorted(heap, args, interns),
            Self::Sum => sum::builtin_sum(heap, args, interns),
            Self::Type => type_::builtin_type(heap, args),
            Self::Zip => zip::builtin_zip(heap, args, interns),
        }
    }
}
