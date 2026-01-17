/// Type definitions for Python runtime values.
///
/// This module contains structured types that wrap heap-allocated data
/// and provide Python-like semantics for operations like append, insert, etc.
///
/// The `AbstractValue` trait provides a common interface for all heap-allocated
/// types, enabling efficient dispatch via `enum_dispatch`.
pub mod bytes;
pub mod dataclass;
pub mod dict;
pub mod list;
pub mod py_trait;
pub mod range;
pub mod set;
pub mod str;
pub mod tuple;
pub mod r#type;

pub(crate) use bytes::Bytes;
pub(crate) use dataclass::Dataclass;
pub(crate) use dict::Dict;
pub(crate) use list::List;
pub(crate) use py_trait::PyTrait;
pub(crate) use range::Range;
pub(crate) use set::{FrozenSet, Set};
pub(crate) use str::Str;
pub(crate) use tuple::Tuple;
pub(crate) use r#type::Type;
