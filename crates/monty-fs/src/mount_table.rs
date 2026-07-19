//! Mount table for mapping virtual paths to host directories.
//!
//! The [`MountTable`] manages a collection of mount points, each mapping a
//! virtual path to a real host directory with a specific access mode.

use std::{
    fs,
    path::{Path, PathBuf},
};

use monty::{MontyObject, OsFunctionCall};

use super::{
    common::MountContext, dispatch, error::MountError, mount_mode::MountMode, path_security::normalize_virtual_path,
};

/// Default aggregate memory budget for one mount: 100 MB in decimal bytes.
pub const DEFAULT_MEMORY_USAGE_LIMIT: u64 = 100_000_000;

/// Outcome of [`MountTable::handle_os_call`].
///
/// The call is consumed so write payloads can be moved into overlay storage;
/// when no mount covers it, ownership is handed back so the caller can
/// surface the call to its fallback handler (host callback, `on_no_handler`).
#[derive(Debug)]
pub enum MountCallOutcome {
    /// A mount covered the call and serviced it (successfully or not).
    Handled(Result<MontyObject, MountError>),
    /// Non-filesystem op or no matching mount — the call, returned unchanged.
    NotHandled(OsFunctionCall),
}

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
    /// boundary checks compare canonical-to-canonical. Mount memory uses
    /// [`DEFAULT_MEMORY_USAGE_LIMIT`] unless a pre-built [`Mount`] overrides it.
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
    /// Use this when a mount was validated before the table was assembled.
    pub fn push_mount(&mut self, mount: Mount) {
        // Keep mounts sorted longest-prefix-first so dispatch can stop at the
        // first match without re-sorting the whole table on every insertion.
        let insert_at = self
            .mounts
            .partition_point(|existing| existing.virtual_path.len() > mount.virtual_path.len());
        self.mounts.insert(insert_at, mount);
    }

    /// Handles an OS call using the mount table.
    ///
    /// Consumes the call so a covered write's payload is *moved* into the
    /// backend (overlay storage retains it without a copy). Routing happens
    /// on a borrow first, so [`MountCallOutcome::NotHandled`] hands the call
    /// back untouched for the caller's fallback handler (a host callback or
    /// [`OsFunctionCall::on_no_handler`]).
    pub fn handle_os_call(&mut self, call: OsFunctionCall) -> MountCallOutcome {
        if call.is_filesystem() {
            match self.route_call(&call) {
                Some(Ok(index)) => MountCallOutcome::Handled(self.mounts[index].execute(call)),
                Some(Err(err)) => MountCallOutcome::Handled(Err(err)),
                None => MountCallOutcome::NotHandled(call),
            }
        } else {
            MountCallOutcome::NotHandled(call)
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

    /// Selects the mount that should handle `call`, routing on borrowed paths
    /// so the call itself stays intact for [`MountCallOutcome::NotHandled`].
    ///
    /// Rename requests require both source and destination to resolve to the
    /// same longest-prefix mount. Other requests only route on the primary path.
    fn route_call(&self, call: &OsFunctionCall) -> Option<Result<usize, MountError>> {
        let primary_path = call.primary_path().expect("filesystem call always has a primary path");
        let src_mount_index = self.find_mount_index(primary_path)?;

        if let Some(dst_path) = call.rename_destination() {
            let dst_mount_index = self.find_mount_index(dst_path)?;
            if src_mount_index != dst_mount_index {
                return Some(Err(MountError::CrossMountRename {
                    src: primary_path.to_owned(),
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
/// [`MountMode::OverlayMemory`] mounts. It can be constructed before its table
/// and transferred into it with [`MountTable::push_mount`].
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
    /// Aggregate budget for retained overlay data and transient results.
    memory_usage_limit: u64,
}

impl Mount {
    /// Creates a new mount point, canonicalizing the host path.
    /// Mount memory defaults to [`DEFAULT_MEMORY_USAGE_LIMIT`].
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
            memory_usage_limit: DEFAULT_MEMORY_USAGE_LIMIT,
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

    /// Returns the aggregate mount memory budget.
    #[must_use]
    pub fn memory_usage_limit(&self) -> u64 {
        self.memory_usage_limit
    }

    /// Overrides the aggregate mount memory budget.
    #[must_use]
    pub fn with_memory_usage_limit(mut self, limit: u64) -> Self {
        self.memory_usage_limit = limit;
        self
    }

    /// Returns memory currently retained by this mount's overlay.
    #[must_use]
    pub fn memory_usage(&self) -> u64 {
        match &self.mode {
            MountMode::OverlayMemory(state) => state.memory_usage(),
            MountMode::ReadWrite | MountMode::ReadOnly => 0,
        }
    }

    /// Returns the cumulative number of bytes written through this mount.
    #[must_use]
    pub fn write_bytes_used(&self) -> u64 {
        self.write_bytes_used
    }

    /// Executes a filesystem call against this mount, consuming it so write
    /// payloads move into the backend.
    fn execute(&mut self, call: OsFunctionCall) -> Result<MontyObject, MountError> {
        let mut ctx = MountContext {
            mount_virtual: &self.virtual_path,
            mount_host: &self.host_path,
            write_bytes_used: &mut self.write_bytes_used,
            write_bytes_limit: self.write_bytes_limit,
            memory_usage_limit: self.memory_usage_limit,
        };
        dispatch::execute(dispatch::fs_request_from_call(call), &mut ctx, &mut self.mode)
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
