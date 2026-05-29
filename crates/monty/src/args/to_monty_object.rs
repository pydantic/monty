//! `ToMontyObject` — owned-value projection into [`crate::MontyObject`].
//!
//! Inverse of [`crate::args::FromValue`]. Used by `#[derive(ToArgs)]` to walk
//! a typed args struct and push each field onto the
//! `(Vec<MontyObject>, Vec<(MontyObject, MontyObject)>)` pair host callbacks
//! consume. Owned-value semantics (consumes `self`) avoid an extra clone for
//! large fields like `String` / `Vec<u8>`.
//!
//! Impls live here for the leaf types that ToArgs structs use directly
//! (`String`, `Vec<u8>`, `bool`, `MontyObject` identity, [`FileMode`]). Each
//! domain newtype (notably [`crate::os::MontyPath`]) implements `ToMontyObject`
//! next to its definition.

use crate::{FileMode, MontyObject};

/// Consume `self` into a [`MontyObject`].
///
/// `MontyObject` is the host-facing, heap-free representation. Implementers
/// just shape themselves into the most natural `MontyObject` variant —
/// `String` → `MontyObject::String`, `Vec<u8>` → `MontyObject::Bytes`, etc.
pub(crate) trait ToMontyObject {
    fn into_monty_object(self) -> MontyObject;
}

impl ToMontyObject for MontyObject {
    fn into_monty_object(self) -> MontyObject {
        self
    }
}

impl ToMontyObject for String {
    fn into_monty_object(self) -> MontyObject {
        MontyObject::String(self)
    }
}

impl ToMontyObject for Vec<u8> {
    fn into_monty_object(self) -> MontyObject {
        MontyObject::Bytes(self)
    }
}

impl ToMontyObject for bool {
    fn into_monty_object(self) -> MontyObject {
        MontyObject::Bool(self)
    }
}

impl ToMontyObject for FileMode {
    fn into_monty_object(self) -> MontyObject {
        MontyObject::String(self.as_str().to_owned())
    }
}
