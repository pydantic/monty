//! Filesystem mounting system for sandboxed execution.
//!
//! This module provides [`MountTable`], which maps virtual paths (as seen by
//! sandbox code) to real host directories with configurable access modes.
//! When sandbox code calls filesystem methods like `Path.read_text()`, the
//! mount table intercepts the operation, resolves the virtual path to a
//! validated host path, and executes the operation according to the mount mode.
//!
//! # Security
//!
//! **CRITICAL: The monty runtime MUST NEVER read, write, or obtain any
//! information about any file or directory outside the specific directory
//! that is mounted.**
//!
//! This invariant is enforced by [`path_security::resolve_path`], which:
//! - Normalizes virtual paths (removing `.` and `..`)
//! - Canonicalizes host paths via [`std::fs::canonicalize`]
//! - Verifies canonical paths remain within the mount boundary
//! - Rejects symlinks that resolve outside the mount
//!
//! # Mount Modes
//!
//! - [`MountMode::ReadWrite`] — full read/write access to the host directory
//! - [`MountMode::ReadOnly`] — reads work, writes raise `PermissionError`
//! - [`MountMode::OverlayMemory`] — reads fall through to host; writes stored in memory
//! - [`MountMode::OverlayDirectory`] — reads fall through; writes go to a separate directory
//!
//! # Example
//!
//! ```no_run
//! use monty::fs::{MountTable, MountMode};
//!
//! let mut mounts = MountTable::new();
//! mounts.mount("/data", "/real/host/data", MountMode::ReadOnly).unwrap();
//! mounts.mount("/tmp", "/real/host/tmp", MountMode::ReadWrite).unwrap();
//!
//! // In the OsCall handler:
//! // match mounts.handle_os_call(&function, &args, &kwargs) {
//! //     Some(Ok(result)) => /* handled */,
//! //     Some(Err(err)) => /* mount error */,
//! //     None => /* not a filesystem op, pass through */,
//! // }
//! ```

mod error;
mod mount_mode;
mod mount_table;
mod operations;
pub mod path_security;

pub use error::MountError;
pub use mount_mode::{MountMode, OverlayEntry, OverlayFile, OverlayState};
pub use mount_table::MountTable;
