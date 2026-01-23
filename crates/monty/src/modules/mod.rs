//! Built-in module implementations.
//!
//! This module provides implementations for Python built-in modules like `sys`, `typing`,
//! and `asyncio`. These are created on-demand when import statements are executed.

use strum::FromRepr;

use crate::{
    args::ArgValues,
    exception_private::RunResult,
    heap::{Heap, HeapId},
    intern::{Interns, StaticStrings, StringId},
    resource::{ResourceError, ResourceTracker},
    value::Value,
};

pub(crate) mod asyncio;
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
}

impl BuiltinModule {
    /// Get the module from a string ID.
    pub fn from_string_id(string_id: StringId) -> Option<Self> {
        match StaticStrings::from_string_id(string_id)? {
            StaticStrings::Sys => Some(Self::Sys),
            StaticStrings::Typing => Some(Self::Typing),
            StaticStrings::Asyncio => Some(Self::Asyncio),
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
        }
    }
}

/// Functions that live inside modules (not global builtins).
///
/// Unlike `BuiltinsFunctions` which are globally available (`print`, `len`),
/// these functions require importing a module to access (`asyncio.gather`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromRepr, strum::Display, serde::Serialize, serde::Deserialize)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum ModuleFunctions {
    /// `asyncio.gather(*coros)` - gather multiple coroutines into a single awaitable.
    #[strum(serialize = "gather")]
    AsyncioGather,
}

impl ModuleFunctions {
    /// Calls this module function with the given arguments.
    pub fn call(self, heap: &mut Heap<impl ResourceTracker>, args: ArgValues) -> RunResult<Value> {
        match self {
            Self::AsyncioGather => asyncio::gather(heap, args),
        }
    }
}
