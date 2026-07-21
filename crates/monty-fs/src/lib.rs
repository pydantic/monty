//! Filesystem mounting system for sandboxed execution.
//!
//! Provides [`MountTable`], which maps virtual paths to real host directories
//! with configurable access modes. When sandbox code calls filesystem methods
//! like `Path.read_text()`, the mount table intercepts the operation, resolves
//! the virtual path, and executes it according to the mount mode.
//!
//! This crate is HOST-side code: it performs real `std::fs` I/O and is linked
//! only by host/parent crates (`monty-pool`, the CLI, bindings). The `monty`
//! interpreter crate deliberately does not depend on it — sandboxed code can
//! only *request* filesystem operations by suspending with an
//! [`OsFunctionCall`](monty::OsFunctionCall), which a host holding a
//! [`MountTable`] services via [`MountTable::handle_os_call`].
//!
//! # Security
//!
//! **The monty runtime MUST NEVER read, write, or obtain any information about
//! any file or directory outside the specific directory that is mounted.**
//!
//! Enforced by `path_security::resolve_path` via path canonicalization,
//! boundary checks, and symlink escape detection.
//! Each mount has an aggregate memory budget, defaulting to
//! [`DEFAULT_MEMORY_USAGE_LIMIT`], for retained overlay data and results.
//!
//! # Mount Modes
//!
//! - [`MountMode::ReadWrite`] — full read/write access to the host directory
//! - [`MountMode::ReadOnly`] — reads work, writes raise `PermissionError`
//! - [`MountMode::OverlayMemory`] — reads fall through to host; writes stored in memory

pub use error::MountError;
pub use mount_mode::MountMode;
pub use mount_table::{DEFAULT_MEMORY_USAGE_LIMIT, Mount, MountCallOutcome, MountTable};
pub use overlay_state::OverlayState;

mod common;
mod direct;
mod dispatch;
mod error;
mod mount_mode;
mod mount_table;
mod overlay;
mod overlay_state;
mod path_security;
