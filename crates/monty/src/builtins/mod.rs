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
mod len;
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
    exception_private::{ExcType, RunResult},
    heap::{DropWithHeap, Heap, HeapData},
    intern::Interns,
    io::PrintWriter,
    resource::ResourceTracker,
    types::{ClassMethod, StaticMethod, Type, UserProperty},
    value::Value,
};
pub(crate) mod isinstance;

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
    Classmethod,
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
    Getattr,
    // Globals,
    Hasattr,
    Hash,
    // Help,
    Hex,
    Id,
    // Input,
    // int - handled by Type enum
    Isinstance,
    Issubclass,
    // Iter - handled by Type enum
    Len,
    // list - handled by Type enum
    // Locals,
    // Map,
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
    Property,
    // range - handled by Type enum
    Repr,
    Reversed,
    Round,
    // set - handled by Type enum
    Setattr,
    // Slice,
    Sorted,
    Staticmethod,
    // str - handled by Type enum
    Sum,
    Super,
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
    pub(crate) fn call(
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
            Self::Issubclass => isinstance::builtin_issubclass(heap, args),
            Self::Len => len::builtin_len(heap, args, interns),
            Self::Max => min_max::builtin_max(heap, args, interns),
            Self::Min => min_max::builtin_min(heap, args, interns),
            Self::Next => next::builtin_next(heap, args, interns),
            Self::Oct => oct::builtin_oct(heap, args),
            Self::Ord => ord::builtin_ord(heap, args, interns),
            Self::Pow => pow::builtin_pow(heap, args),
            Self::Print => print::builtin_print(heap, args, interns, print_writer),
            Self::Repr => repr::builtin_repr(heap, args, interns),
            Self::Reversed => reversed::builtin_reversed(heap, args, interns),
            Self::Round => round::builtin_round(heap, args),
            Self::Sorted => sorted::builtin_sorted(heap, args, interns),
            Self::Sum => sum::builtin_sum(heap, args, interns),
            Self::Type => type_::builtin_type(heap, args, interns),
            Self::Zip => zip::builtin_zip(heap, args, interns),
            Self::Super => {
                // super() is handled specially in the VM (needs frame context)
                args.drop_with_heap(heap);
                Err(ExcType::type_error(
                    "super() must be called from within a method".to_string(),
                ))
            }
            Self::Staticmethod => builtin_staticmethod(heap, args),
            Self::Classmethod => builtin_classmethod(heap, args),
            Self::Property => builtin_property(heap, args),
            Self::Getattr | Self::Setattr | Self::Hasattr => {
                // These need mutable Interns for dynamic string interning.
                // Handled specially in the VM's call_function dispatch.
                args.drop_with_heap(heap);
                Err(ExcType::type_error(
                    "getattr/setattr/hasattr must be called via VM dispatch".to_string(),
                ))
            }
        }
    }
}

/// `staticmethod(func)` - wraps a function as a static method.
fn builtin_staticmethod(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let func = args.get_one_arg("staticmethod", heap)?;
    let sm = StaticMethod::new(func);
    let id = heap.allocate(HeapData::StaticMethod(sm))?;
    Ok(Value::Ref(id))
}

/// `classmethod(func)` - wraps a function as a class method.
fn builtin_classmethod(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let func = args.get_one_arg("classmethod", heap)?;
    let cm = ClassMethod::new(func);
    let id = heap.allocate(HeapData::ClassMethod(cm))?;
    Ok(Value::Ref(id))
}

/// `property(fget=None)` - creates a property descriptor.
fn builtin_property(heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
    let fget = args.get_zero_one_arg("property", heap)?;
    let up = UserProperty::new(fget);
    let id = heap.allocate(HeapData::UserProperty(up))?;
    Ok(Value::Ref(id))
}
