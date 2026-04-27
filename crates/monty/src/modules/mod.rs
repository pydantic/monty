//! Built-in module implementations.
//!
//! This module provides implementations for Python built-in modules like `sys`, `typing`,
//! and `asyncio`. These are created on-demand when import statements are executed.

use std::fmt::{self, Write};

use strum::FromRepr;

use crate::{
    args::ArgValues,
    bytecode::{CallResult, VM},
    exception_private::RunResult,
    heap::HeapId,
    intern::{StaticStrings, StringId},
    resource::{ResourceError, ResourceTracker},
};

pub(crate) mod asyncio;
pub(crate) mod datetime;
#[cfg(feature = "test-hooks")]
pub(crate) mod gc;
pub(crate) mod json;
pub(crate) mod math;
pub(crate) mod os;
pub(crate) mod pathlib;
pub(crate) mod re;
pub(crate) mod sys;
pub(crate) mod typing;

/// Built-in modules that can be imported.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromRepr)]
pub(crate) enum StandardLib {
    /// The `sys` module providing system-specific parameters and functions.
    Sys,
    /// The `typing` module providing type hints support.
    Typing,
    /// The `asyncio` module providing async/await support (only `gather()` implemented).
    Asyncio,
    /// The `pathlib` module providing object-oriented filesystem paths.
    Pathlib,
    /// The `os` module providing operating system interface (only `getenv()` implemented).
    Os,
    /// The `math` module providing mathematical functions and constants.
    Math,
    /// The `json` module providing JSON parsing and serialization.
    Json,
    /// The `re` module providing regular expression matching.
    Re,
    /// The `datetime` module providing date and time types.
    Datetime,
    /// The `gc` module exposing a single `collect()` for tests. Only present
    /// under the `test-hooks` feature so production sandboxes never see it.
    ///
    /// The variant is gated rather than left as a permanent unused entry so the
    /// `from_repr` <-> discriminant mapping doesn't carry a hole on production
    /// builds. Because it's the last variant, gating it has no effect on the
    /// numeric discriminants of any other module.
    #[cfg(feature = "test-hooks")]
    Gc,
}

impl StandardLib {
    /// Get the module from a string ID.
    pub fn from_string_id(string_id: StringId) -> Option<Self> {
        match StaticStrings::from_string_id(string_id)? {
            StaticStrings::Sys => Some(Self::Sys),
            StaticStrings::Typing => Some(Self::Typing),
            StaticStrings::Asyncio => Some(Self::Asyncio),
            StaticStrings::Pathlib => Some(Self::Pathlib),
            StaticStrings::Os => Some(Self::Os),
            StaticStrings::Math => Some(Self::Math),
            StaticStrings::Json => Some(Self::Json),
            StaticStrings::Re => Some(Self::Re),
            StaticStrings::Datetime => Some(Self::Datetime),
            #[cfg(feature = "test-hooks")]
            StaticStrings::Gc => Some(Self::Gc),
            _ => None,
        }
    }

    /// Creates a new instance of this module on the heap.
    ///
    /// Returns a HeapId pointing to the newly allocated module.
    ///
    /// # Panics
    ///
    /// Panics if the required strings have not been pre-interned during prepare phase.
    pub fn create(self, vm: &mut VM<'_, '_, impl ResourceTracker>) -> Result<HeapId, ResourceError> {
        match self {
            Self::Sys => sys::create_module(vm),
            Self::Typing => typing::create_module(vm),
            Self::Asyncio => asyncio::create_module(vm),
            Self::Pathlib => pathlib::create_module(vm),
            Self::Os => os::create_module(vm),
            Self::Math => math::create_module(vm),
            Self::Json => json::create_module(vm),
            Self::Re => re::create_module(vm),
            Self::Datetime => datetime::create_module(vm),
            #[cfg(feature = "test-hooks")]
            Self::Gc => gc::create_module(vm),
        }
    }
}

/// All stdlib module function (but not builtins).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) enum ModuleFunctions {
    Asyncio(asyncio::AsyncioFunctions),
    Json(json::JsonFunctions),
    Math(math::MathFunctions),
    Os(os::OsFunctions),
    Re(re::ReFunctions),
    /// `gc` module functions — only present under the `test-hooks` feature.
    /// See [`gc`] for why we keep this gated rather than always-on.
    #[cfg(feature = "test-hooks")]
    Gc(gc::GcFunctions),
}

impl fmt::Display for ModuleFunctions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Asyncio(func) => write!(f, "{func}"),
            Self::Json(func) => write!(f, "{func}"),
            Self::Math(func) => write!(f, "{func}"),
            Self::Os(func) => write!(f, "{func}"),
            Self::Re(func) => write!(f, "{func}"),
            #[cfg(feature = "test-hooks")]
            Self::Gc(func) => write!(f, "{func}"),
        }
    }
}

impl ModuleFunctions {
    /// Calls the module function with the given arguments.
    ///
    /// Returns `CallResult` to support both immediate values and OS calls that
    /// require host involvement (e.g., `os.getenv()` needs the host to provide environment variables).
    pub fn call(self, vm: &mut VM<'_, '_, impl ResourceTracker>, args: ArgValues) -> RunResult<CallResult> {
        match self {
            Self::Asyncio(functions) => asyncio::call(vm.heap, functions, args),
            Self::Json(functions) => json::call(vm, functions, args).map(CallResult::Value),
            Self::Math(functions) => math::call(vm, functions, args).map(CallResult::Value),
            Self::Os(functions) => os::call(vm, functions, args),
            Self::Re(functions) => re::call(vm, functions, args),
            #[cfg(feature = "test-hooks")]
            Self::Gc(functions) => gc::call(vm, functions, args).map(CallResult::Value),
        }
    }

    /// Writes the Python repr() string for this function to a formatter.
    pub fn py_repr_fmt<W: Write>(self, f: &mut W, py_id: usize) -> fmt::Result {
        write!(f, "<function {self} at 0x{py_id:x}>")
    }
}
