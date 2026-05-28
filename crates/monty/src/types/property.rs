//! Python property descriptor for computed attributes.
//!
//! Properties are descriptors whose value is computed when accessed.
//! When a Property is retrieved via `py_getattr`, its getter is invoked
//! rather than returning the Property itself.

use crate::{bytecode::CallResult, os::OsFunctionCall};

/// Property descriptor for computed attributes (mirrors Python's descriptor
/// protocol — accessing the property invokes its getter).
///
/// Currently only supports zero-arg OS properties (e.g. `os.environ`).
/// Future variants will likely add `Callable(FunctionId)` for `@property`
/// and `External(StringId)` for external function getters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) enum Property {
    Os(ZeroArgOsProperty),
}

/// Discriminant for zero-arg OS-backed [`Property`]s. Kept `Copy` so
/// `Property` stays `Copy + Hash`; the matching [`OsFunctionCall`] (which
/// is not `Copy`) is built on access in [`Property::get`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) enum ZeroArgOsProperty {
    /// `os.environ` — returns the host environment as a dict.
    GetEnviron,
}

impl Property {
    /// Invokes the getter, returning the `CallResult` the VM should act on.
    pub fn get(self) -> CallResult {
        match self {
            Self::Os(ZeroArgOsProperty::GetEnviron) => CallResult::OsCall(OsFunctionCall::GetEnviron),
        }
    }
}
