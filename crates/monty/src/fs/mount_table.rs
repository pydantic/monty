//! Mount table for mapping virtual paths to host directories.
//!
//! The [`MountTable`] manages a collection of mount points, each mapping a
//! virtual path to a real host directory with a specific access mode.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use super::{
    common::MountContext,
    dispatch::{self, FsRequest},
    error::MountError,
    mount_mode::MountMode,
    path_security::normalize_virtual_path,
};
use crate::{MontyObject, os::OsFunction};

/// A collection of mount points mapping virtual paths to host directories.
///
/// Mounts are checked in longest-prefix-first order so that more specific
/// mounts take precedence.
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
    pub fn mount(
        &mut self,
        virtual_path: &str,
        host_path: impl AsRef<Path>,
        mode: MountMode,
        write_bytes_limit: Option<u64>,
    ) -> Result<(), MountError> {
        let mount = Mount::new(virtual_path, host_path, mode, write_bytes_limit)?;
        self.push_mount(mount);
        Ok(())
    }

    /// Adds a pre-built [`Mount`] to the table.
    ///
    /// Use this when a `Mount` was constructed elsewhere (e.g. owned by a Python
    /// `MountDirectory` and temporarily taken for the duration of a run).
    pub fn push_mount(&mut self, mount: Mount) {
        // Keep mounts sorted longest-prefix-first so dispatch can stop at the
        // first match without re-sorting the whole table on every insertion.
        let insert_at = self
            .mounts
            .partition_point(|existing| existing.virtual_path.len() > mount.virtual_path.len());
        self.mounts.insert(insert_at, mount);
    }

    /// Consumes this table and returns all mounts.
    #[must_use]
    pub fn into_mounts(self) -> Vec<Mount> {
        self.mounts
    }

    /// Takes all mounts out of shared slots and assembles a [`MountTable`].
    ///
    /// Each slot is `Arc<Mutex<Option<Mount>>>`. The mount is taken via
    /// `Option::take` so the slot becomes `None` during execution. Use
    /// [`put_back_shared_mounts`] to restore them after the run completes.
    ///
    /// # Errors
    ///
    /// Returns an error message if any mutex is poisoned or any mount is
    /// already taken (concurrent use).
    pub fn take_shared_mounts(slots: &[Arc<Mutex<Option<Mount>>>]) -> Result<Self, String> {
        let mut table = Self::new();
        for (i, shared) in slots.iter().enumerate() {
            let mut guard = shared.lock().map_err(|_| format!("mount {i} lock is poisoned"))?;
            let mount = guard
                .take()
                .ok_or_else(|| format!("mount {i} is already in use by another run"))?;
            table.push_mount(mount);
        }
        Ok(table)
    }

    /// Puts all mounts back into their shared slots after execution completes.
    ///
    /// Must be called after every [`take_shared_mounts`](Self::take_shared_mounts),
    /// even on error paths, to avoid permanently losing the mounts.
    pub fn put_back_shared_mounts(self, slots: &[Arc<Mutex<Option<Mount>>>]) {
        for (shared, mount) in slots.iter().zip(self.into_mounts()) {
            if let Ok(mut slot) = shared.lock() {
                debug_assert!(slot.is_none(), "mount slot should be empty during put_back");
                *slot = Some(mount);
            }
        }
    }

    /// Handles an OS call using the mount table.
    ///
    /// Returns `Some(Ok(result))` if handled, `Some(Err(..))` on error, or
    /// `None` if the operation was not handled (non-filesystem op, or no
    /// matching mount for the path). The caller should fall through to a
    /// callback or use [`OsFunction::on_no_handler`] for unhandled calls.
    pub fn handle_os_call(
        &mut self,
        function: OsFunction,
        args: &[MontyObject],
        kwargs: &[(MontyObject, MontyObject)],
    ) -> Option<Result<MontyObject, MountError>> {
        if !function.is_filesystem() {
            return None;
        }

        let request = match dispatch::parse_fs_request(function, args, kwargs) {
            Ok(request) => request,
            Err(err) => return Some(Err(err)),
        };

        match self.route_request(request) {
            Some(Ok(index)) => Some(self.mounts[index].execute(request)),
            Some(Err(err)) => Some(Err(err)),
            None => None,
        }
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

    /// Selects the mount that should handle `request`.
    ///
    /// Rename requests require both source and destination to resolve to the
    /// same longest-prefix mount. Other requests only route on the primary path.
    fn route_request(&self, request: FsRequest<'_>) -> Option<Result<usize, MountError>> {
        let src_mount_index = self.find_mount_index(request.primary_path())?;

        if let Some(dst_path) = request.rename_destination() {
            let dst_mount_index = self.find_mount_index(dst_path)?;
            if src_mount_index != dst_mount_index {
                return Some(Err(MountError::CrossMountRename {
                    src: request.primary_path().to_owned(),
                    dst: dst_path.to_owned(),
                }));
            }
        }

        Some(Ok(src_mount_index))
    }

    /// Finds the longest-prefix mount index for `virtual_path`.
    fn find_mount_index(&self, virtual_path: &str) -> Option<usize> {
        let normalized = normalize_virtual_path(virtual_path);
        self.mounts
            .iter()
            .position(|mount| path_matches_mount(&normalized, &mount.virtual_path))
    }
}

/// A single mount point mapping a virtual path to a host directory.
///
/// Owns the [`MountMode`] which includes overlay state for
/// [`MountMode::OverlayMemory`] mounts. Can be stored externally (e.g. in a
/// Python `MountDirectory`) and temporarily moved into a [`MountTable`] for
/// the duration of execution via [`MountTable::push_mount`] /
/// [`MountTable::take_shared_mounts`].
#[derive(Debug)]
pub struct Mount {
    /// Virtual path prefix (absolute, normalized).
    virtual_path: String,
    /// Canonical host directory path (resolved at construction time).
    host_path: PathBuf,
    /// Access mode (also owns overlay state for [`MountMode::OverlayMemory`]).
    mode: MountMode,
    /// Cumulative bytes written through this mount (monotonically increasing).
    write_bytes_used: u64,
    /// Optional cap on cumulative bytes written. When exceeded, writes raise `OSError`.
    write_bytes_limit: Option<u64>,
}

impl Mount {
    /// Creates a new mount point, canonicalizing the host path.
    ///
    /// # Errors
    ///
    /// Returns [`MountError::InvalidMount`] if the virtual path is not absolute,
    /// or the host path doesn't exist or isn't a directory.
    pub fn new(
        virtual_path: &str,
        host_path: impl AsRef<Path>,
        mode: MountMode,
        write_bytes_limit: Option<u64>,
    ) -> Result<Self, MountError> {
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
            write_bytes_used: 0,
            write_bytes_limit,
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

    /// Returns the optional write bytes limit for this mount.
    #[must_use]
    pub fn write_bytes_limit(&self) -> Option<u64> {
        self.write_bytes_limit
    }

    /// Returns the cumulative number of bytes written through this mount.
    #[must_use]
    pub fn write_bytes_used(&self) -> u64 {
        self.write_bytes_used
    }

    /// Executes a parsed filesystem request against this mount.
    fn execute(&mut self, request: FsRequest<'_>) -> Result<MontyObject, MountError> {
        let mut ctx = MountContext {
            mount_virtual: &self.virtual_path,
            mount_host: &self.host_path,
            write_bytes_used: &mut self.write_bytes_used,
            write_bytes_limit: self.write_bytes_limit,
        };
        dispatch::execute(request, &mut ctx, &mut self.mode)
    }
}

/// Checks whether `normalized_path` falls under `mount_virtual_path`.
fn path_matches_mount(normalized_path: &str, mount_virtual_path: &str) -> bool {
    if mount_virtual_path == "/" || normalized_path == mount_virtual_path {
        true
    } else {
        normalized_path.starts_with(mount_virtual_path)
            && normalized_path.as_bytes().get(mount_virtual_path.len()) == Some(&b'/')
    }
}
