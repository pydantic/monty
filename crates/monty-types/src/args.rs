//! [`ToArgs`] / [`ToMontyObject`] — projection of typed args structs into
//! the `(positional, keyword)` [`MontyObject`] pairs host callbacks consume.
//! The `#[derive(ToArgs)]` macro in `monty-macros` emits impls of these
//! traits via `crate::args::…` paths, which resolve in this crate.

use num_bigint::BigInt;

use crate::{file_mode::FileMode, object::MontyObject};
/// Projects a typed args struct into the `(positional, keyword)` [`MontyObject`]
/// pair host callbacks expect. Consumes `self` to avoid cloning owned fields.
///
/// Inverse of `monty`'s internal `FromArgs` (`ArgValues` → struct); [`ToArgs`]
/// is struct → host-facing `(args, kwargs)`. Driven by
/// [`crate::os::OsFunctionCall::to_args`] for the monty-python / monty-js bindings.
pub trait ToArgs {
    fn to_args(self) -> (Vec<MontyObject>, Vec<(MontyObject, MontyObject)>);
}
/// Consume `self` into a [`MontyObject`].
///
/// [`MontyObject`] is the host-facing, heap-free representation. Implementers
/// just shape themselves into the most natural [`MontyObject`] variant —
/// `String` → [`MontyObject::String`], `Vec<u8>` → [`MontyObject::Bytes`], etc.
pub trait ToMontyObject {
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

impl ToMontyObject for i64 {
    fn into_monty_object(self) -> MontyObject {
        MontyObject::Int(self)
    }
}

/// Counts above `i64::MAX` cross as `BigInt`, so a host handler receives the
/// exact value and its own cap decides what to do with it.
impl ToMontyObject for u64 {
    fn into_monty_object(self) -> MontyObject {
        i64::try_from(self).map_or_else(|_| MontyObject::BigInt(BigInt::from(self)), MontyObject::Int)
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
