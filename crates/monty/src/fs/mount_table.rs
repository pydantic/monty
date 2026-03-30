//! Mount table for mapping virtual paths to host directories.
//!
//! The [`MountTable`] manages a collection of mount points, each mapping a virtual
//! path (as seen by sandbox code) to a real host directory with a specific access
//! mode. When sandbox code performs a filesystem operation, the mount table:
//!
//! 1. Identifies the matching mount by longest-prefix match
//! 2. Resolves the virtual path to a validated host path
//! 3. Executes the operation according to the mount's access mode
//!
//! # Security
//!
//! **The monty runtime MUST NEVER read, write, or obtain any information about
//! any file or directory outside the specific directory that is mounted.**
//!
//! All path resolution goes through [`super::path_security::resolve_path`] which
//! enforces path canonicalization, boundary checks, and symlink escape protection.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{
    error::MountError,
    mount_mode::MountMode,
    operations::{self, MountContext},
    path_security::normalize_virtual_path,
};
use crate::{MontyObject, os::OsFunction};

/// A single mount point mapping a virtual path to a host directory.
///
/// The `virtual_path` is what sandbox code sees (e.g., `/data`).
/// The `host_path` is the canonical real directory on the host filesystem.
#[derive(Debug)]
struct Mount {
    /// The virtual path prefix as seen by sandbox code.
    /// Always absolute and normalized (no `.` or `..`).
    virtual_path: String,

    /// The canonical host directory path. Canonicalized at mount time so
    /// that all boundary checks compare canonical-to-canonical.
    host_path: PathBuf,

    /// Access mode controlling read/write behavior. For [`MountMode::OverlayMemory`],
    /// this also owns the in-memory overlay state.
    mode: MountMode,
}

/// A collection of mount points that map virtual paths to host directories.
///
/// When sandbox code performs a filesystem operation, the `MountTable` resolves
/// the virtual path to a real host path (if any mount matches), applies security
/// checks, and executes the operation according to the mount mode.
///
/// Mounts are checked in longest-prefix-first order so that more specific mounts
/// take precedence over less specific ones.
///
/// # Security
///
/// **CRITICAL:** The monty runtime MUST NEVER read, write, or obtain any
/// information about any file or directory outside the specific directory that
/// is mounted. This is enforced by:
///
/// - Path canonicalization after mapping virtual → host paths
/// - Boundary checks verifying canonical paths remain within the mount
/// - Symlink resolution that rejects links pointing outside the mount
/// - Virtual-space normalization that prevents `..` escape
/// - `Resolve` and `Absolute` returning virtual paths, never host paths
///
/// # Example
///
/// ```no_run
/// use monty::fs::{MountTable, MountMode};
///
/// let mut mounts = MountTable::new();
/// mounts.mount("/data", "/real/host/data", MountMode::ReadOnly).unwrap();
/// mounts.mount("/tmp", "/real/host/tmp", MountMode::ReadWrite).unwrap();
/// ```
#[derive(Debug, Default)]
pub struct MountTable {
    /// Mount points sorted by `virtual_path` length descending (longest first)
    /// for correct prefix matching.
    mounts: Vec<Mount>,
}

impl MountTable {
    /// Creates a new empty mount table with no mount points.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a mount point mapping a virtual path to a host directory.
    ///
    /// The virtual path must be absolute (start with `/`). The host path must
    /// exist and be a directory. The host path is canonicalized at mount time.
    ///
    /// For [`MountMode::OverlayDirectory`], the upper directory must also exist.
    ///
    /// # Errors
    ///
    /// Returns [`MountError::InvalidMount`] if:
    /// - The virtual path is not absolute
    /// - The host path doesn't exist or isn't a directory
    /// - The host path cannot be canonicalized
    /// - For `OverlayDirectory`: the upper directory doesn't exist
    ///
    /// # Security
    ///
    /// The host path is canonicalized once here. All subsequent boundary checks
    /// compare against this canonical path, ensuring symlink changes after mount
    /// time cannot bypass the boundary.
    pub fn mount(
        &mut self,
        virtual_path: &str,
        host_path: impl AsRef<Path>,
        mode: MountMode,
    ) -> Result<(), MountError> {
        let host_path = host_path.as_ref();

        // Validate virtual path.
        if !virtual_path.starts_with('/') {
            return Err(MountError::InvalidMount(format!(
                "virtual path must be absolute, got: '{virtual_path}'"
            )));
        }

        let normalized_virtual = normalize_virtual_path(virtual_path);

        // Canonicalize and validate host path.
        let canonical_host = fs::canonicalize(host_path).map_err(|e| {
            MountError::InvalidMount(format!(
                "cannot canonicalize host path '{}': {}",
                host_path.display(),
                e
            ))
        })?;

        if !canonical_host.is_dir() {
            return Err(MountError::InvalidMount(format!(
                "host path is not a directory: '{}'",
                host_path.display()
            )));
        }

        // Validate upper directory for OverlayDirectory mode.
        if let MountMode::OverlayDirectory { ref upper_dir } = mode {
            let canonical_upper = fs::canonicalize(upper_dir).map_err(|e| {
                MountError::InvalidMount(format!(
                    "cannot canonicalize upper directory '{}': {e}",
                    upper_dir.display()
                ))
            })?;
            if !canonical_upper.is_dir() {
                return Err(MountError::InvalidMount(format!(
                    "upper directory is not a directory: '{}'",
                    upper_dir.display()
                )));
            }
        }

        self.mounts.push(Mount {
            virtual_path: normalized_virtual,
            host_path: canonical_host,
            mode,
        });

        // Re-sort by virtual path length descending (longest prefix first).
        self.mounts
            .sort_by(|a, b| b.virtual_path.len().cmp(&a.virtual_path.len()));

        Ok(())
    }

    /// Attempts to handle a filesystem [`OsCall`] using the mount table.
    ///
    /// Returns `Some(Ok(result))` if the operation was handled successfully,
    /// `Some(Err(mount_err))` if there was an error, or `None` if the
    /// [`OsFunction`] is not a filesystem operation (meaning the caller should
    /// pass it through to the existing host callback).
    ///
    /// # Arguments
    ///
    /// * `function` — The OS function to execute
    /// * `args` — Positional arguments (first is typically `MontyObject::Path`)
    /// * `kwargs` — Keyword arguments (e.g., `parents` and `exist_ok` for mkdir)
    pub fn handle_os_call(
        &mut self,
        function: OsFunction,
        args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> Option<Result<MontyObject, MountError>> {
        // Non-filesystem operations pass through.
        if !function.is_filesystem() {
            return None;
        }

        // Extract the virtual path from the first argument.
        let virtual_path = match args.first() {
            Some(MontyObject::Path(p)) => p.as_str(),
            Some(MontyObject::String(s)) => s.as_str(),
            _ => {
                return Some(Err(MountError::InvalidMount(
                    "filesystem operation missing path argument".to_owned(),
                )));
            }
        };

        // Special case for Rename: validate that both paths are in the same mount.
        if matches!(function, OsFunction::Rename)
            && let Some(result) = self.handle_rename(virtual_path, &args[1..], kwargs)
        {
            return Some(result);
        }

        // Find the matching mount (longest prefix first).
        let normalized = normalize_virtual_path(virtual_path);
        let extra_args = if args.len() > 1 { &args[1..] } else { &[] };

        let mount = self.find_mount_mut(&normalized)?;
        let ctx = MountContext {
            mount_virtual: &mount.virtual_path.clone(),
            mount_host: &mount.host_path.clone(),
        };
        let result = operations::execute(function, virtual_path, extra_args, kwargs, &ctx, &mut mount.mode);
        Some(result)
    }

    /// Returns `true` if this mount table has any mount points configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mounts.is_empty()
    }

    /// Returns the number of configured mount points.
    #[must_use]
    pub fn len(&self) -> usize {
        self.mounts.len()
    }

    /// Finds the mount whose virtual path is a prefix of the given normalized path.
    ///
    /// Returns the longest matching mount (mounts are pre-sorted by length descending).
    fn find_mount_mut(&mut self, normalized_path: &str) -> Option<&mut Mount> {
        self.mounts.iter_mut().find(|m| {
            if m.virtual_path == "/" {
                true
            } else {
                normalized_path == m.virtual_path || normalized_path.starts_with(&format!("{}/", m.virtual_path))
            }
        })
    }

    /// Handles rename across potentially different mount points.
    ///
    /// Returns `None` if both paths are in the same mount (handled by normal dispatch).
    /// Returns `Some(Err)` if the paths are in different mounts (cross-device link error).
    fn handle_rename(
        &mut self,
        src_virtual: &str,
        extra_args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> Option<Result<MontyObject, MountError>> {
        let dst_virtual = match extra_args.first() {
            Some(MontyObject::Path(p)) => p.as_str(),
            Some(MontyObject::String(s)) => s.as_str(),
            _ => return None,
        };

        let src_normalized = normalize_virtual_path(src_virtual);
        let dst_normalized = normalize_virtual_path(dst_virtual);

        let src_mount_idx = self.mounts.iter().position(|m| {
            if m.virtual_path == "/" {
                true
            } else {
                src_normalized == m.virtual_path || src_normalized.starts_with(&format!("{}/", m.virtual_path))
            }
        });
        let dst_mount_idx = self.mounts.iter().position(|m| {
            if m.virtual_path == "/" {
                true
            } else {
                dst_normalized == m.virtual_path || dst_normalized.starts_with(&format!("{}/", m.virtual_path))
            }
        });

        match (src_mount_idx, dst_mount_idx) {
            (Some(s), Some(d)) if s == d => {
                // Same mount — let normal dispatch handle it.
                let mount = &mut self.mounts[s];
                let ctx = MountContext {
                    mount_virtual: &mount.virtual_path.clone(),
                    mount_host: &mount.host_path.clone(),
                };
                let result = operations::execute(
                    OsFunction::Rename,
                    src_virtual,
                    extra_args,
                    kwargs,
                    &ctx,
                    &mut mount.mode,
                );
                Some(result)
            }
            (Some(_), Some(_)) => Some(Err(MountError::CrossMountRename {
                src: src_virtual.to_owned(),
                dst: dst_virtual.to_owned(),
            })),
            _ => None, // One or both paths not in any mount.
        }
    }
}
