//! Python property descriptor for computed attributes.
//!
//! Properties are descriptors whose value is computed when accessed.
//! When a Property is retrieved via `py_getattr`, its getter is invoked
//! rather than returning the Property itself.

use monty_types::OsFunctionCall;

use crate::bytecode::CallResult;

/// Property descriptor for computed attributes (mirrors Python's descriptor
/// protocol — accessing the property invokes its getter).
///
/// Only zero-arg OS properties on modules (e.g. `os.environ`). User-level
/// `property` objects are heap [`Descriptor`](super::Descriptor)s instead.
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
    /// The dotted name the property is reached by; its hash key in dicts and sets.
    pub fn name(self) -> &'static str {
        match self {
            Self::Os(ZeroArgOsProperty::GetEnviron) => "os.environ",
        }
    }

    /// Invokes the getter, returning the `CallResult` the VM should act on.
    pub fn get(self) -> CallResult {
        match self {
            Self::Os(ZeroArgOsProperty::GetEnviron) => CallResult::OsCall(OsFunctionCall::GetEnviron),
        }
    }
}
