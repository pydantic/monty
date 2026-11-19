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
pub(crate) mod eval_exec;
mod filter;
mod format;
mod getattr;
mod hasattr;
mod hash;
mod hex;
mod id;
pub(crate) mod isinstance;
pub(crate) mod issubclass;
pub(crate) mod iter;
mod len;
mod locals;
mod map;
mod min_max; // min and max share implementation
pub(crate) use min_max::candidate_wins;
pub(crate) mod next;
pub(crate) mod object;
pub(crate) mod object_setattr;
mod oct;
pub(crate) mod open;
mod ord;
mod pow;
mod print;
mod repr;
mod reversed;
mod round;
mod setattr;
mod sorted;
mod sum;
pub(crate) mod type_;
mod zip;

use std::{fmt, fmt::Write, str::FromStr};

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::{ExcType, RunResult},
    types::Type,
    value::Value,
};

/// A builtin callable stored inline in a [`Value`]: a function or a native type.
///
/// Dumps name the variants, so `Function` and `Type` are the persistence
/// contract; [`DumpedBuiltins`] reads the shapes older builds wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(from = "DumpedBuiltins")]
pub(crate) enum Builtins {
    /// A builtin function like `print` or `len`.
    Function(BuiltinsFunctions),
    /// A native type like `list`, `int` or `ValueError`, which is also its constructor.
    Type(Type),
}

/// Every shape a stored [`Builtins`] has had: exception classes were a separate
/// `ExcType` variant, and `type` was a builtin function, before both joined the
/// native type table.
#[derive(serde::Deserialize)]
enum DumpedBuiltins {
    Function(BuiltinsFunctions),
    ExcType(ExcType),
    Type(Type),
}

impl From<DumpedBuiltins> for Builtins {
    fn from(dumped: DumpedBuiltins) -> Self {
        match dumped {
            DumpedBuiltins::Function(BuiltinsFunctions::Type) => Self::Type(Type::Type),
            DumpedBuiltins::Function(function) => Self::Function(function),
            DumpedBuiltins::ExcType(exc) => Self::Type(Type::from(exc)),
            DumpedBuiltins::Type(ty) => Self::Type(ty),
        }
    }
}

impl Builtins {
    /// Resolves builtin names that evaluate directly to singleton values.
    #[must_use]
    pub fn value_from_name(name: &str) -> Option<Value> {
        match name {
            "Ellipsis" => Some(Value::Ellipsis),
            "NotImplemented" => Some(Value::NotImplemented),
            _ => name.parse::<Self>().ok().map(Value::Builtin),
        }
    }

    /// Calls this builtin, allowing builtins that need host involvement to yield.
    ///
    /// Most builtins produce a [`CallResult::Value`]. `iter()` and `next()` can
    /// push frames for user methods; `open()` yields its host filesystem operation,
    /// and `next()` on an unread file yields the read that loads its buffer.
    pub fn call(self, vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
        match self {
            Self::Function(b) => b.call(vm, args),
            Self::Type(t) => t.call(vm, args),
        }
    }

    /// Writes the Python repr() string for this callable to a formatter.
    pub fn py_repr_fmt<W: Write>(self, f: &mut W) -> fmt::Result {
        match self {
            Self::Function(b) => write!(f, "<built-in function {b}>"),
            Self::Type(t) => write!(f, "<class '{t}'>"),
        }
    }

    /// Returns the type of this builtin.
    pub fn py_type(self) -> Type {
        match self {
            Self::Function(_) => Type::BuiltinFunction,
            Self::Type(_) => Type::Type,
        }
    }

    /// The bytes `id()` encodes: the postcard form of the pre-table enum, which
    /// had `Function`, `ExcType` and `Type` variants, with `type` a function, so
    /// `id(len)`, `id(type)`, `id(ValueError)` and `id(int)` survive a restore.
    pub(crate) fn identity_key(self) -> (u32, IdentityPayload) {
        match self {
            Self::Function(function) => (0, IdentityPayload::Function(function)),
            Self::Type(Type::Type) => (0, IdentityPayload::Function(BuiltinsFunctions::Type)),
            Self::Type(ty) => match ty.exc_type() {
                Some(exc_type) => (1, IdentityPayload::ExcType(exc_type)),
                None => (2, IdentityPayload::Type(ty)),
            },
        }
    }
}

/// The payload half of [`Builtins::identity_key`].
#[derive(serde::Serialize)]
#[serde(untagged)]
pub(crate) enum IdentityPayload {
    Function(BuiltinsFunctions),
    ExcType(ExcType),
    Type(Type),
}

impl FromStr for Builtins {
    type Err = ();

    /// Resolves a name in the builtins namespace. Types win over functions so
    /// `type` is the type object; `BuiltinsFunctions::Type` survives only as the
    /// bytecode operand class bodies call.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(t) = Type::from_builtin_name(s) {
            Ok(Self::Type(t))
        } else if let Ok(b) = BuiltinsFunctions::from_str(s) {
            Ok(Self::Function(b))
        } else {
            Err(())
        }
    }
}

pub use monty_types::BuiltinsFunctions;
pub(crate) use type_::type_of_ref;

pub(crate) trait BuiltinsFunctionsExt: Sized {
    fn call(self, vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult>;
}

impl BuiltinsFunctionsExt for BuiltinsFunctions {
    fn call(self, vm: &mut VM<'_>, args: ArgValues) -> RunResult<CallResult> {
        let r = match self {
            Self::Abs => abs::builtin_abs(vm, args),
            Self::All => all::builtin_all(vm, args),
            Self::Any => any::builtin_any(vm, args),
            Self::Bin => bin::builtin_bin(vm, args),
            Self::Chr => chr::builtin_chr(vm, args),
            Self::Divmod => divmod::builtin_divmod(vm, args),
            Self::Enumerate => enumerate::builtin_enumerate(vm, args),
            Self::Filter => filter::builtin_filter(vm, args),
            Self::Format => format::builtin_format(vm, args),
            // `getattr()` / `hasattr()` may suspend a lazy host attribute lookup.
            Self::Getattr => return getattr::builtin_getattr(vm, args),
            Self::Hasattr => return hasattr::builtin_hasattr(vm, args),
            Self::Hash => hash::builtin_hash(vm, args),
            Self::Hex => hex::builtin_hex(vm, args),
            Self::Id => id::builtin_id(vm, args),
            Self::Isinstance => isinstance::builtin_isinstance(vm, args),
            Self::Issubclass => issubclass::builtin_issubclass(vm, args),
            Self::Len => len::builtin_len(vm, args),
            Self::Map => map::builtin_map(vm, args),
            Self::Max => min_max::builtin_max(vm, args),
            Self::Min => min_max::builtin_min(vm, args),
            // `next()` on an unread file yields the read that loads it.
            Self::Next => return next::builtin_next(vm, args),
            Self::Oct => oct::builtin_oct(vm, args),
            // `open()` yields an OS call rather than a plain value.
            Self::Open => return open::builtin_open(vm, args),
            Self::Ord => ord::builtin_ord(vm, args),
            Self::Pow => pow::builtin_pow(vm, args),
            Self::Print => print::builtin_print(vm, args),
            Self::Repr => repr::builtin_repr(vm, args),
            Self::Reversed => reversed::builtin_reversed(vm, args),
            Self::Round => round::builtin_round(vm, args),
            Self::Setattr => setattr::builtin_setattr(vm, args),
            Self::ObjectSetattr => object_setattr::builtin_object_setattr(vm, args),
            Self::ObjectNew => object::builtin_object_new(vm, args),
            Self::ObjectInit => object::builtin_object_init(vm, args),
            Self::ObjectInitSubclass => object::builtin_object_init_subclass(vm, args),
            Self::TypeMro => type_::builtin_type_mro(vm, args),
            Self::Sorted => sorted::builtin_sorted(vm, args),
            Self::Sum => sum::builtin_sum(vm, args),
            Self::Type => type_::builtin_type(vm, args),
            Self::Zip => zip::builtin_zip(vm, args),
            // `eval()` / `exec()` push the compiled snippet's frame.
            Self::Eval => return eval_exec::builtin_eval(vm, args),
            Self::Exec => return eval_exec::builtin_exec(vm, args),
            Self::Locals => locals::builtin_locals(vm, args),
        };
        r.map(CallResult::Value)
    }
}
