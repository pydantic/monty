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

pub use bytes::Bytes;
pub use dataclass::Dataclass;
pub use dict::Dict;
pub use list::List;
pub use py_trait::PyTrait;
pub use r#type::Type;
pub use range::Range;
pub use set::{FrozenSet, Set};
pub use str::Str;
pub use tuple::Tuple;
