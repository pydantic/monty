//! Test-only entry points for invariant fuzzing.
//!
//! Unstable and not part of the public API: the fuzz crate imports these to
//! exercise path policy without a mount or any host I/O.

use crate::{
    MountError,
    path_security::{reject_overlong_path, resolve_virtual_path},
};

/// Applies Monty's path policy to `virtual_path` as if a mount were at `/mnt`.
///
/// Mirrors the order [`MountTable::handle_os_call`](crate::MountTable::handle_os_call)
/// uses before routing: length first (the only O(1) rejection), then null bytes
/// and normalization inside [`resolve_virtual_path`]. Returns the mount-relative
/// path, which is empty for the mount root itself.
///
/// A caller can therefore assert the confinement invariant on `Ok`: the result
/// has no `..` component, no backslash, no Windows drive prefix, and no null
/// byte, so joining it to the mount root cannot climb out.
pub fn resolve_mounted_path(virtual_path: &str) -> Result<String, MountError> {
    reject_overlong_path(virtual_path)?;
    resolve_virtual_path(virtual_path, "/mnt").map(|path| path.as_relative_str().to_owned())
}
