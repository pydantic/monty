//! Mount table for mapping virtual paths to host directories.
//!
//! The [`MountTable`] manages a collection of mount points, each mapping a
//! virtual path to a real host directory with a specific access mode.

use std::{
    cmp::Reverse,
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

/// A collection of mount points mapping virtual paths to host directories.
///
/// Mounts are checked in longest-prefix-first order so that more specific
/// mounts take precedence.
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
    /// Sorted by `virtual_path` length descending (longest first).
    mounts: Vec<Mount>,
}

impl MountTable {
    /// Creates a new empty mount table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a mount point mapping a virtual path to a host directory.
    ///
    /// The host path is canonicalized at mount time so that all subsequent
    /// boundary checks compare canonical-to-canonical.
    ///
    /// # Errors
    ///
    /// Returns [`MountError::InvalidMount`] if the virtual path is not absolute,
    /// or the host path doesn't exist or isn't a directory.
    /// Adds a mount point mapping a virtual path to a host directory.
    ///
    /// The host path is canonicalized at mount time so that all subsequent
    /// boundary checks compare canonical-to-canonical.
    pub fn mount(
        &mut self,
        virtual_path: &str,
        host_path: impl AsRef<Path>,
        mode: MountMode,
    ) -> Result<(), MountError> {
        let mount = Mount::new(virtual_path, host_path, mode)?;
        self.push_mount(mount);
        Ok(())
    }

    /// Adds a pre-built [`Mount`] to the table.
    ///
    /// Use this when a `Mount` was constructed elsewhere (e.g. owned by a Python
    /// `MountDirectory` and temporarily taken for the duration of a run).
    pub fn push_mount(&mut self, mount: Mount) {
        self.mounts.push(mount);
        // Re-sort: longest prefix first for correct matching.
        self.mounts.sort_by_key(|m| Reverse(m.virtual_path.len()));
    }

    /// Get and returns all mounts.
    #[must_use]
    pub fn get_mounts(self) -> Vec<Mount> {
        self.mounts
    }

    /// Handles an OS call using the mount table.
    ///
    /// Filesystem operations are dispatched to the matching mount point.
    /// Returns `None` for non-filesystem operations (e.g. `os.getenv`) so
    /// the caller can try a fallback callback.
    ///
    /// Filesystem ops with no matching mount return [`MountError::NoMountPoint`]
    /// which maps to `PermissionError`.
    pub fn handle_os_call(
        &mut self,
        function: OsFunction,
        args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> Option<Result<MontyObject, MountError>> {
        if !function.is_filesystem() {
            return None;
        }

        let virtual_path = match args.first() {
            Some(MontyObject::Path(p)) => p.as_str(),
            Some(MontyObject::String(s)) => s.as_str(),
            _ => {
                return Some(Err(MountError::InvalidMount(
                    "filesystem operation missing path argument".to_owned(),
                )));
            }
        };

        // Rename needs special handling: both paths must be in the same mount.
        if matches!(function, OsFunction::Rename)
            && let Some(result) = self.handle_rename(virtual_path, &args[1..], kwargs)
        {
            return Some(result);
        }

        let normalized = normalize_virtual_path(virtual_path);
        let extra_args = if args.len() > 1 { &args[1..] } else { &[] };

        let Some(mount) = self.find_mount_mut(&normalized) else {
            return Some(Err(MountError::NoMountPoint(virtual_path.to_owned())));
        };
        Some(mount.execute(function, virtual_path, extra_args, kwargs))
    }

    /// Returns `true` if no mount points are configured.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mounts.is_empty()
    }

    /// Returns the number of configured mount points.
    #[must_use]
    pub fn len(&self) -> usize {
        self.mounts.len()
    }

    /// Finds the mount whose virtual path is the longest prefix of `normalized_path`.
    fn find_mount_mut(&mut self, normalized_path: &str) -> Option<&mut Mount> {
        self.mounts
            .iter_mut()
            .find(|m| path_matches_mount(normalized_path, &m.virtual_path))
    }

    /// Handles rename, validating both paths are in the same mount.
    ///
    /// Returns `None` to let normal dispatch handle same-mount renames that
    /// weren't resolved here (e.g., missing dst path argument).
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

        let src_mount_idx = self
            .mounts
            .iter()
            .position(|m| path_matches_mount(&src_normalized, &m.virtual_path));
        let dst_mount_idx = self
            .mounts
            .iter()
            .position(|m| path_matches_mount(&dst_normalized, &m.virtual_path));

        match (src_mount_idx, dst_mount_idx) {
            (Some(s), Some(d)) if s == d => {
                Some(self.mounts[s].execute(OsFunction::Rename, src_virtual, extra_args, kwargs))
            }
            (Some(_), Some(_)) => Some(Err(MountError::CrossMountRename {
                src: src_virtual.to_owned(),
                dst: dst_virtual.to_owned(),
            })),
            _ => None,
        }
    }
}

// =============================================================================
// Mount — a single mount point
// =============================================================================

/// A single mount point mapping a virtual path to a host directory.
///
/// Owns the [`MountMode`] which includes overlay state for
/// [`MountMode::OverlayMemory`] mounts. Can be stored externally (e.g. in a
/// Python `MountDirectory`) and temporarily moved into a [`MountTable`] for
/// the duration of execution via [`MountTable::push_mount`] /
/// [`MountTable::take_mounts`].
#[derive(Debug)]
pub struct Mount {
    /// Virtual path prefix (absolute, normalized).
    virtual_path: String,
    /// Canonical host directory path (resolved at construction time).
    host_path: PathBuf,
    /// Access mode (also owns overlay state for [`MountMode::OverlayMemory`]).
    mode: MountMode,
}

impl Mount {
    /// Creates a new mount point, canonicalizing the host path.
    ///
    /// # Errors
    ///
    /// Returns [`MountError::InvalidMount`] if the virtual path is not absolute,
    /// or the host path doesn't exist or isn't a directory.
    pub fn new(virtual_path: &str, host_path: impl AsRef<Path>, mode: MountMode) -> Result<Self, MountError> {
        let host_path = host_path.as_ref();

        if !virtual_path.starts_with('/') {
            return Err(MountError::InvalidMount(format!(
                "virtual path must be absolute, got: '{virtual_path}'"
            )));
        }

        let normalized_virtual = normalize_virtual_path(virtual_path);

        let canonical_host = fs::canonicalize(host_path).map_err(|e| {
            MountError::InvalidMount(format!("cannot canonicalize host path '{}': {e}", host_path.display()))
        })?;

        if !canonical_host.is_dir() {
            return Err(MountError::InvalidMount(format!(
                "host path is not a directory: '{}'",
                host_path.display()
            )));
        }

        Ok(Self {
            virtual_path: normalized_virtual,
            host_path: canonical_host,
            mode,
        })
    }

    /// Returns the normalized virtual path prefix for this mount.
    #[must_use]
    pub fn virtual_path(&self) -> &str {
        &self.virtual_path
    }

    /// Returns the canonical host directory path.
    #[must_use]
    pub fn host_path(&self) -> &Path {
        &self.host_path
    }

    /// Returns the access mode for this mount.
    #[must_use]
    pub fn mode(&self) -> &MountMode {
        &self.mode
    }

    /// Executes a filesystem operation against this mount.
    ///
    /// Builds a [`MountContext`] by borrowing `virtual_path` and `host_path`
    /// while passing `mode` as `&mut`, avoiding unnecessary clones.
    pub fn execute(
        &mut self,
        function: OsFunction,
        virtual_path: &str,
        extra_args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> Result<MontyObject, MountError> {
        let ctx = MountContext {
            mount_virtual: &self.virtual_path,
            mount_host: &self.host_path,
        };
        operations::execute(function, virtual_path, extra_args, kwargs, &ctx, &mut self.mode)
    }
}

/// Checks whether `normalized_path` falls under `mount_virtual_path`.
fn path_matches_mount(normalized_path: &str, mount_virtual_path: &str) -> bool {
    if mount_virtual_path == "/" {
        return true;
    }
    normalized_path == mount_virtual_path || normalized_path.starts_with(&format!("{mount_virtual_path}/"))
}
