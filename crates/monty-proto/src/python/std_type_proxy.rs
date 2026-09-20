//! The read-only stand-in for a builtin function or type object the sandbox
//! hands out. Sandbox output never materialises the host's own `open`,
//! `exec` or `io.TextIOWrapper`: a name crosses, wrapped in a proxy.

use std::{
    fmt,
    hash::{DefaultHasher, Hash, Hasher},
};

use monty_types::{BuiltinsFunctions, MontyType, StringRepr, unstable::MontyNode};
use pyo3::prelude::*;

/// Read-only proxy for a builtin function, or a type object outside the
/// host-class allowlist (see `host_type_object`), returned from the sandbox.
/// Only the name crosses, so the host never holds a live callable built from
/// sandbox output; passed back in, the proxy re-enters as the builtin itself.
#[pyclass(name = "MontyStdTypeProxy", module = "pydantic_monty", frozen)]
pub struct PyMontyStdTypeProxy {
    /// The builtin it stands for, kept typed so it crosses back losslessly.
    pub(super) inner: StdTypeRef,
}

#[pymethods]
impl PyMontyStdTypeProxy {
    /// `'function'` for a builtin function, `'type'` for a type object.
    #[getter]
    fn kind(&self) -> &'static str {
        match self.inner {
            StdTypeRef::Type(_) => "type",
            StdTypeRef::Function(_) => "function",
        }
    }

    /// The name the sandbox renders the builtin as (`'open'`, `'functools.partial'`).
    #[getter]
    fn name(&self) -> String {
        self.inner.to_string()
    }

    /// `MontyStdTypeProxy(kind='function', name='open')`
    fn __repr__(&self) -> String {
        format!(
            "MontyStdTypeProxy(kind={}, name={})",
            StringRepr(self.kind()),
            StringRepr(&self.name())
        )
    }

    /// Equal when standing for the same builtin.
    fn __eq__(&self, other: &Bound<'_, PyAny>) -> bool {
        other
            .extract::<PyRef<'_, Self>>()
            .is_ok_and(|other| self.inner == other.inner)
    }

    /// Hashes the name, which is unique across both kinds.
    fn __hash__(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.name().hash(&mut hasher);
        hasher.finish()
    }
}

/// What a [`PyMontyStdTypeProxy`] stands for.
#[derive(PartialEq)]
pub(super) enum StdTypeRef {
    Type(MontyType),
    Function(BuiltinsFunctions),
}

impl StdTypeRef {
    /// The node the proxy crosses back into the sandbox as.
    pub(super) fn to_node(&self) -> MontyNode {
        match self {
            Self::Type(t) => MontyNode::Type(t.clone()),
            Self::Function(f) => MontyNode::BuiltinFunction(*f),
        }
    }
}

impl fmt::Display for StdTypeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Type(t) => t.fmt(f),
            Self::Function(func) => func.fmt(f),
        }
    }
}
