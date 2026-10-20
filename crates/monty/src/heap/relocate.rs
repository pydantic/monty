//! Typed wire reconstruction helpers; only the owner supplies live references.

use super::{HeapIndex, RuntimeRef};
use crate::{heap_data::BoxOn32, value::Value};

impl Value<HeapIndex> {
    /// Rebuilds direct and indirect references without imposing RC cleanup on wire values.
    pub(crate) fn relocate<H: RuntimeRef>(self, reference: &mut impl FnMut(HeapIndex) -> H) -> Value<H> {
        match &self {
            Self::Ref(index) => Value::Ref(reference(*index)),
            Self::Builtin(builtin) => Value::Builtin(builtin.relocate(reference)),
            _ => self.copy_without_refs().expect("transient value in snapshot"),
        }
    }
}

/// Preserves target-dependent boxing while rebuilding a payload.
#[cfg(target_pointer_width = "32")]
#[expect(
    clippy::boxed_local,
    reason = "consumes boxed wire storage while changing its reference representation"
)]
pub(crate) fn map_box_on_32<T, U>(value: BoxOn32<T>, map: impl FnOnce(T) -> U) -> BoxOn32<U> {
    Box::new(map(*value))
}

/// Native payloads remain inline during reconstruction.
#[cfg(not(target_pointer_width = "32"))]
pub(crate) fn map_box_on_32<T, U>(value: BoxOn32<T>, map: impl FnOnce(T) -> U) -> BoxOn32<U> {
    map(value)
}
