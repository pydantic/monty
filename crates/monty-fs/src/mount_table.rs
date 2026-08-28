//! Mount table for mapping virtual paths to host directories.
//!
//! The [`MountTable`] manages a collection of mount points, each mapping a
//! virtual path to a real host directory with a specific access mode.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use cap_std::{ambient_authority, fs::Dir};
use monty_types::{MontyObject, OsFunctionCall};

use super::{
    common::MountContext,
    dispatch,
    error::MountError,
    mount_mode::MountMode,
    path_security::{contains_null_byte, normalize_virtual_path, reject_overlong_path},
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
    /// The host directory is opened once here, and every later operation runs
    /// relative to that descriptor — so the mount stays attached to the
    /// directory that was named, whatever the host does to the path afterwards.
    /// Mount memory uses [`DEFAULT_MEMORY_USAGE_LIMIT`] unless a pre-built
    /// [`Mount`] overrides it.
    ///
    /// With [`MountMode::ReadWrite`], files written by sandboxed code persist
    /// on the host. Read that variant's warning before choosing it.
    ///
    /// # Errors
    ///
    /// Returns [`MountError::InvalidMount`] if the virtual path is not absolute,
    /// the host path doesn't exist or isn't a directory, or it cannot be opened
    /// — on macOS/BSD that includes a search-only (`0o111`) directory, which
    /// Linux accepts because it opens directories with `O_PATH`.
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
            .partition_point(|existing| existing.virtual_path().len() > mount.virtual_path().len());
        self.mounts.insert(insert_at, mount);
    }

    /// Handles an OS call using the mount table.
    ///
    /// Consumes the call so a covered write's payload is *moved* into the
    /// backend (overlay storage retains it without a copy). Routing happens
    /// on a borrow first, so [`MountCallOutcome::NotHandled`] hands the call
    /// back untouched for the caller's fallback handler (a host callback or
    /// [`OsFunctionCall::on_no_handler`]).
    ///
    /// Path length and null bytes are checked before anything else touches the
    /// path, so both apply whether or not a mount covers it — as in CPython,
    /// where neither reaches a syscall.
    pub fn handle_os_call(&mut self, call: OsFunctionCall) -> MountCallOutcome {
        if let Some(primary_path) = call.fs_primary_path() {
            // Length first: it is the only check that stays O(1) on a hostile
            // path, so a null scan must not run ahead of it. A path that is
            // both reports its length, where CPython reports the null byte.
            let rejection = reject_overlong_path(primary_path).err().or_else(|| {
                contains_null_byte(primary_path)
                    .then(|| MountError::EmbeddedNullByte(call.embedded_null_message(false)))
            });
            if let Some(e) = rejection {
                // Both make CPython's predicates answer `False` rather than
                // raise — `pathlib` swallows `OSError` and `ValueError` alike.
                MountCallOutcome::Handled(if call.is_existence_check() {
                    Ok(MontyObject::Bool(false))
                } else {
                    Err(e)
                })
            } else {
                match self.route_call(primary_path, &call) {
                    Some(Ok(index)) => MountCallOutcome::Handled(self.mounts[index].execute(call)),
                    Some(Err(err)) => MountCallOutcome::Handled(Err(err)),
                    None => MountCallOutcome::NotHandled(call),
                }
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
    ///
    /// One side covered and the other not is refused rather than handed on: the
    /// fallback answers on raw virtual paths, skipping the mount's access mode.
    fn route_call(&self, primary_path: &str, call: &OsFunctionCall) -> Option<Result<usize, MountError>> {
        let src_mount_index = self.find_mount_index(primary_path);

        if let Some(dst_path) = call.rename_destination() {
            // The destination gets the same pre-routing checks the source had
            // above, so an unusable name is refused even when neither side is
            // mounted. `in dst` is what tells the two apart to the caller.
            if let Err(e) = reject_overlong_path(dst_path) {
                return Some(Err(e));
            }
            if contains_null_byte(dst_path) {
                return Some(Err(MountError::EmbeddedNullByte(call.embedded_null_message(true))));
            }
            match (src_mount_index, self.find_mount_index(dst_path)) {
                // Neither side is ours; the whole call belongs to the fallback.
                (None, None) => None,
                (Some(src), Some(dst)) if src == dst => Some(Ok(src)),
                _ => Some(Err(MountError::CrossMountRename {
                    src: primary_path.to_owned(),
                    dst: dst_path.to_owned(),
                })),
            }
        } else {
            src_mount_index.map(Ok)
        }
    }

    /// Finds the longest-prefix mount index for `virtual_path`.
    fn find_mount_index(&self, virtual_path: &str) -> Option<usize> {
        let normalized = normalize_virtual_path(virtual_path);
        self.mounts
            .iter()
            .position(|mount| path_matches_mount(&normalized, mount.virtual_path()))
    }
}

/// A single mount point mapping a virtual path to a host directory.
///
/// Owns the [`MountMode`] which includes overlay state for
/// [`MountMode::OverlayMemory`] mounts. It can be constructed before its table
/// and transferred into it with [`MountTable::push_mount`].
#[derive(Debug)]
pub struct Mount {
    /// The opened directory this mount serves, and the virtual path it answers on.
    root: MountRoot,
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
    /// Creates a new mount point, opening a descriptor on the host directory.
    /// Mount memory defaults to [`DEFAULT_MEMORY_USAGE_LIMIT`].
    ///
    /// A host mounting the same directory repeatedly should open a [`MountRoot`]
    /// once and use [`Mount::with_root`], resolving the name only once.
    ///
    /// # Errors
    ///
    /// Returns [`MountError::InvalidMount`] if the virtual path is not absolute,
    /// or the host path cannot be opened as a directory or canonicalized.
    pub fn new(
        virtual_path: &str,
        host_path: impl AsRef<Path>,
        mode: MountMode,
        write_bytes_limit: Option<u64>,
    ) -> Result<Self, MountError> {
        Ok(Self::with_root(
            MountRoot::open(virtual_path, host_path)?,
            mode,
            write_bytes_limit,
        ))
    }

    /// Mounts an already-opened [`MountRoot`], touching no filesystem at all.
    /// Mount memory defaults to [`DEFAULT_MEMORY_USAGE_LIMIT`].
    #[must_use]
    pub fn with_root(root: MountRoot, mode: MountMode, write_bytes_limit: Option<u64>) -> Self {
        Self {
            root,
            mode,
            write_bytes_used: 0,
            write_bytes_limit,
            memory_usage_limit: DEFAULT_MEMORY_USAGE_LIMIT,
        }
    }

    /// Returns the opened root, to clone into a later mount of the same directory.
    #[must_use]
    pub fn root(&self) -> &MountRoot {
        &self.root
    }

    /// Returns the normalized virtual path prefix for this mount.
    #[must_use]
    pub fn virtual_path(&self) -> &str {
        self.root.virtual_path()
    }

    /// Returns the canonical host directory path. Diagnostics only.
    #[must_use]
    pub fn host_path(&self) -> &Path {
        self.root.host_path()
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
            mount_virtual: &self.root.virtual_path,
            mount_dir: &self.root.dir,
            write_bytes_used: &mut self.write_bytes_used,
            write_bytes_limit: self.write_bytes_limit,
            memory_usage_limit: self.memory_usage_limit,
        };
        dispatch::execute(dispatch::fs_request_from_call(call), &mut ctx, &mut self.mode)
    }
}

/// A host directory opened once, mountable as often as the host likes; cloning
/// shares the descriptor.
///
/// Reuse one instead of re-deriving a mount from its path: sandbox code that
/// can rename inside a parent mount redirects that name between rebuilds, and
/// an open descriptor cannot be redirected.
#[derive(Debug, Clone)]
pub struct MountRoot {
    /// Virtual path prefix (absolute, normalized).
    virtual_path: String,
    /// Canonical host directory path. Diagnostics only — see `dir`.
    host_path: PathBuf,
    /// Descriptor for the mounted directory — the sandbox boundary, which
    /// resolution cannot leave. Shared, so every mount built from this root is
    /// the same directory rather than the same name resolved again.
    dir: Arc<Dir>,
}

impl MountRoot {
    /// Opens `host_path`, pinning the root to the directory that is there now.
    ///
    /// # Errors
    ///
    /// Returns [`MountError::InvalidMount`] if the virtual path is not absolute,
    /// or the host path cannot be opened as a directory or canonicalized.
    pub fn open(virtual_path: &str, host_path: impl AsRef<Path>) -> Result<Self, MountError> {
        let host_path = host_path.as_ref();

        if !virtual_path.starts_with('/') {
            return Err(MountError::InvalidMount(format!(
                "virtual path must be absolute, got: '{virtual_path}'"
            )));
        }

        let normalized_virtual = normalize_virtual_path(virtual_path);

        // The only use of ambient authority, and the mount's whole trust root.
        // Deliberately first: resolving the name to a validated path and *then*
        // opening that path would let whoever can rename in the parent swap a
        // symlink into the gap. The directory check rides on the open itself
        // (`O_DIRECTORY`, a handle `metadata()` on Windows), so it cannot.
        let dir = Dir::open_ambient_dir(host_path, ambient_authority())
            .map_err(|e| MountError::InvalidMount(format!("cannot open host path '{}': {e}", host_path.display())))?;

        // Diagnostics only — nothing resolves through this path. Resolved after
        // the open, so a host racing it leaves a stale label on the right
        // descriptor, never the reverse. Still fatal on failure: callers copy
        // this out as a mount's durable identity, and a relative path would
        // later re-resolve against the process CWD.
        let canonical_host = fs::canonicalize(host_path).map_err(|e| {
            MountError::InvalidMount(format!("cannot resolve host path '{}': {e}", host_path.display()))
        })?;

        Ok(Self {
            virtual_path: normalized_virtual,
            host_path: canonical_host,
            dir: Arc::new(dir),
        })
    }

    /// Returns the normalized virtual path prefix this root answers on.
    #[must_use]
    pub fn virtual_path(&self) -> &str {
        &self.virtual_path
    }

    /// Returns the canonical host directory path. Diagnostics only.
    #[must_use]
    pub fn host_path(&self) -> &Path {
        &self.host_path
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
