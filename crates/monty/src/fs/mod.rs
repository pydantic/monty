//! Filesystem mounting system for sandboxed execution.
//!
//! Provides [`MountTable`], which maps virtual paths to real host directories
//! with configurable access modes. When sandbox code calls filesystem methods
//! like `Path.read_text()`, the mount table intercepts the operation, resolves
//! the virtual path, and executes it according to the mount mode.
//!
//! # Security
//!
//! **The monty runtime MUST NEVER read, write, or obtain any information about
//! any file or directory outside the specific directory that is mounted.**
//!
//! Enforced by [`path_security::resolve_path`] via path canonicalization,
//! boundary checks, and symlink escape detection.
//!
//! # Mount Modes
//!
//! - [`MountMode::ReadWrite`] — full read/write access to the host directory
//! - [`MountMode::ReadOnly`] — reads work, writes raise `PermissionError`
//! - [`MountMode::OverlayMemory`] — reads fall through to host; writes stored in memory

pub use error::MountError;
pub use mount_mode::MountMode;
pub use mount_table::{Mount, MountTable};
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
