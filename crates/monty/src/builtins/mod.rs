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
mod filter;
mod getattr;
mod hash;
mod hex;
mod id;
mod isinstance;
mod len;
mod map;
mod min_max; // min and max share implementation
mod next;
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

use strum::{Display, EnumString, FromRepr, IntoStaticStr};

use crate::{
    args::ArgValues,
    bytecode::VM,
    exception_private::{ExcType, RunResult},
    resource::ResourceTracker,
    types::Type,
    value::Value,
};

/// Enumerates every interpreter-native Python builtins
///
/// Uses strum derives for automatic `Display`, `FromStr`, and `AsRef<str>` implementations.
/// All variants serialize to lowercase (e.g., `Print` -> "print").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) enum Builtins {
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
    pub fn call(self, vm: &mut VM<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        match self {
            Self::Function(b) => b.call(vm, args),
            Self::ExcType(exc) => exc.call(vm.heap, args, vm.interns),
            Self::Type(t) => t.call(vm.heap, args, vm.interns),
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
}

impl FromStr for Builtins {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Priority: BuiltinsFunctions > ExcType > Type
        // Only matches names that are true Python builtins (accessible without imports).
        if let Ok(b) = BuiltinsFunctions::from_str(s) {
            Ok(Self::Function(b))
        } else if let Ok(exc) = ExcType::from_str(s) {
            Ok(Self::ExcType(exc))
        } else if let Some(t) = Type::from_builtin_name(s) {
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
    Debug,
    Clone,
    Copy,
    Display,
    EnumString,
    FromRepr,
    IntoStaticStr,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
)]
#[strum(serialize_all = "lowercase")]
#[repr(u8)]
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
    Filter,
    // float - handled by Type enum
    // Format,
    // frozenset - handled by Type enum
    Getattr,
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
    // Iter - handled by Type enum
    Len,
    // list - handled by Type enum
    // Locals,
    Map,
    Max,
    // memoryview - handled by Type enum
    Min,
    Next,
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
    pub(crate) fn call(self, vm: &mut VM<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        match self {
            Self::Abs => abs::builtin_abs(vm.heap, args),
            Self::All => all::builtin_all(vm.heap, args, vm.interns),
            Self::Any => any::builtin_any(vm.heap, args, vm.interns),
            Self::Bin => bin::builtin_bin(vm.heap, args),
            Self::Chr => chr::builtin_chr(vm.heap, args),
            Self::Divmod => divmod::builtin_divmod(vm.heap, args),
            Self::Enumerate => enumerate::builtin_enumerate(vm.heap, args, vm.interns),
            Self::Filter => filter::builtin_filter(vm, args),
            Self::Getattr => getattr::builtin_getattr(vm.heap, args, vm.interns),
            Self::Hash => hash::builtin_hash(vm.heap, args, vm.interns),
            Self::Hex => hex::builtin_hex(vm.heap, args),
            Self::Id => id::builtin_id(vm.heap, args),
            Self::Isinstance => isinstance::builtin_isinstance(vm.heap, args),
            Self::Len => len::builtin_len(vm.heap, args, vm.interns),
            Self::Map => map::builtin_map(vm, args),
            Self::Max => min_max::builtin_max(vm.heap, args, vm.interns),
            Self::Min => min_max::builtin_min(vm.heap, args, vm.interns),
            Self::Next => next::builtin_next(vm.heap, args, vm.interns),
            Self::Oct => oct::builtin_oct(vm.heap, args),
            Self::Ord => ord::builtin_ord(vm.heap, args, vm.interns),
            Self::Pow => pow::builtin_pow(vm.heap, args),
            Self::Print => print::builtin_print(vm.heap, args, vm.interns, vm.print_writer),
            Self::Repr => repr::builtin_repr(vm.heap, args, vm.interns),
            Self::Reversed => reversed::builtin_reversed(vm.heap, args, vm.interns),
            Self::Round => round::builtin_round(vm.heap, args),
            Self::Sorted => sorted::builtin_sorted(vm, args),
            Self::Sum => sum::builtin_sum(vm.heap, args, vm.interns),
            Self::Type => type_::builtin_type(vm.heap, args),
            Self::Zip => zip::builtin_zip(vm.heap, args, vm.interns),
        }
    }
}
