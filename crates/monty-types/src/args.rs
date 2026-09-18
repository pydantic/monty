//! Projection of typed argument structs into the [`CallArgs`] host callbacks consume.
//! The `#[derive(ToArgs)]` macro in `monty-macros` emits implementations via
//! `crate::args::ToArgs`, which resolves in this crate.

use crate::object::CallArgs;

/// Projects a typed args struct into the [`CallArgs`] host callbacks expect.
/// Consumes `self` to avoid cloning owned fields.
///
/// Inverse of `monty`'s internal `FromArgs` (`ArgValues` → struct); driven by
/// [`crate::os::OsFunctionCall::to_args`] for the Python and JavaScript bindings.
pub trait ToArgs {
    /// Consumes the fields into arguments for delivery to the host.
    fn to_args(self) -> CallArgs;
}
