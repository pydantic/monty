//! Built-in module implementations.
//!
//! This module provides implementations for Python built-in modules like `sys`, `typing`,
//! and `asyncio`. These are created on-demand when import statements are executed.

use std::fmt::{self, Write};

use strum::FromRepr;

use crate::{
    args::ArgValues,
    exception_private::RunResult,
    heap::{Heap, HeapId},
    intern::{Interns, StaticStrings, StringId},
    resource::{ResourceError, ResourceTracker},
    types::AttrCallResult,
};

pub(crate) mod asyncio;
pub(crate) mod os;
pub(crate) mod pathlib;
pub(crate) mod sys;
pub(crate) mod typing;

/// Built-in modules that can be imported.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromRepr)]
pub(crate) enum BuiltinModule {
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
}

impl BuiltinModule {
    /// Get the module from a string ID.
    pub fn from_string_id(string_id: StringId) -> Option<Self> {
        match StaticStrings::from_string_id(string_id)? {
            StaticStrings::Sys => Some(Self::Sys),
            StaticStrings::Typing => Some(Self::Typing),
            StaticStrings::Asyncio => Some(Self::Asyncio),
            StaticStrings::Pathlib => Some(Self::Pathlib),
            StaticStrings::Os => Some(Self::Os),
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
    pub fn create(self, heap: &mut Heap<impl ResourceTracker>, interns: &Interns) -> Result<HeapId, ResourceError> {
        match self {
            Self::Sys => sys::create_module(heap, interns),
            Self::Typing => typing::create_module(heap, interns),
            Self::Asyncio => asyncio::create_module(heap, interns),
            Self::Pathlib => pathlib::create_module(heap, interns),
            Self::Os => os::create_module(heap, interns),
        }
    }
}

/// All stdlib module function (but not builtins).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) enum ModuleFunctions {
    Asyncio(asyncio::AsyncioFunctions),
    Os(os::OsFunctions),
}

impl fmt::Display for ModuleFunctions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Asyncio(func) => write!(f, "{func}"),
            Self::Os(func) => write!(f, "{func}"),
        }
    }
}

impl ModuleFunctions {
    /// Calls the module function with the given arguments.
    ///
    /// Returns `AttrCallResult` to support both immediate values and OS calls that
    /// require host involvement (e.g., `os.getenv()` needs the host to provide environment variables).
    pub fn call(self, heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<AttrCallResult> {
        match self {
            Self::Asyncio(functions) => asyncio::call(heap, functions, args),
            Self::Os(functions) => os::call(heap, functions, args),
        }
    }

    /// Writes the Python repr() string for this function to a formatter.
    pub fn py_repr_fmt<W: Write>(self, f: &mut W, py_id: usize) -> std::fmt::Result {
        write!(f, "<function {self} at 0x{py_id:x}>")
    }
}
