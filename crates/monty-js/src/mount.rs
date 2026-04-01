//! JavaScript/TypeScript bindings for filesystem mount configuration.
//!
//! Exposes [`MountDirectory`] (a single mount point with shared overlay state)
//! and [`OsHandler`] (a collection of mounts for filesystem operations).
//! Filesystem operations are handled entirely in Rust via the core
//! [`monty::fs::MountTable`], with no JavaScript round-trip.
//!
//! # Take/put pattern
//!
//! [`MountDirectory`] owns its [`Mount`] behind `Arc<Mutex<Option<Mount>>>`.
//! When `Monty.run()` or `Monty.start()` begins, each mount is **taken** out
//! of its slot (zero-cost `Option::take`), moved into a plain [`MountTable`],
//! and execution proceeds with no locking overhead. When the run finishes
//! (or when a snapshot is finalized), each mount is **put back**.

use std::sync::{Arc, Mutex};

use monty::fs::{Mount, MountMode, MountTable};
use napi::{bindgen_prelude::*, JsValue};
use napi_derive::napi;

/// Shared storage for a [`Mount`] that can be temporarily taken for execution.
pub(crate) type SharedMount = Arc<Mutex<Option<Mount>>>;

/// Wraps a `Vec<SharedMount>` for passing between functions.
///
/// Since napi `#[napi]` structs cannot be generic and we need to extract
/// shared mount references from JS arguments before constructing the handler,
/// this type bundles the extracted mounts.
pub(crate) struct ExtractedMounts(pub Vec<SharedMount>);

// =============================================================================
// MountDirectory — owns a shared Mount
// =============================================================================

/// Options for creating a new MountDirectory.
#[napi(object)]
#[derive(Default)]
pub struct MountDirectoryOptions {
    /// Access mode: `'read-only'`, `'read-write'`, or `'overlay'` (default).
    pub mode: Option<String>,
    /// Optional limit on cumulative bytes written through this mount.
    pub write_bytes_limit: Option<i64>,
}

/// A single mount point mapping a virtual path to a host directory.
///
/// Owns the underlying [`Mount`] (including overlay state for `'overlay'` mode)
/// via shared storage, so reusing it across `Monty.run()` calls preserves the
/// same overlay data.
///
/// The `mode` controls sandbox access:
/// - `'read-only'` — sandbox can read but not write
/// - `'read-write'` — sandbox can read and write real host files
/// - `'overlay'` — reads fall through to host; writes are captured in memory
#[napi]
pub struct MountDirectory {
    /// Shared mount storage. `None` while a run is in progress.
    pub(crate) shared: SharedMount,
}

#[napi]
impl MountDirectory {
    /// Creates a new mount directory.
    ///
    /// @param virtualPath - Absolute virtual path prefix (e.g. `"/data"`)
    /// @param hostPath - Path to the real host directory
    /// @param options - Optional mode and write_bytes_limit
    #[napi(constructor)]
    pub fn new(virtual_path: String, host_path: String, options: Option<MountDirectoryOptions>) -> Result<Self> {
        let options = options.unwrap_or_default();
        let mode_str = options.mode.as_deref().unwrap_or("overlay");
        let mount_mode = MountMode::from_mode_str(mode_str).map_err(|e| Error::new(Status::InvalidArg, e))?;
        let write_bytes_limit = match options.write_bytes_limit {
            Some(v) if v < 0 => {
                return Err(Error::new(Status::InvalidArg, "write_bytes_limit must be non-negative"));
            }
            #[expect(clippy::cast_sign_loss)]
            Some(v) => Some(v as u64),
            None => None,
        };

        let mount = Mount::new(&virtual_path, &host_path, mount_mode, write_bytes_limit)
            .map_err(|e| Error::new(Status::InvalidArg, e.into_exception().to_string()))?;

        Ok(Self {
            shared: Arc::new(Mutex::new(Some(mount))),
        })
    }

    /// The normalized virtual path prefix inside the sandbox.
    #[napi(getter)]
    pub fn virtual_path(&self) -> Result<String> {
        self.with_mount(|m| m.virtual_path().to_owned())
    }

    /// The canonical host directory path.
    #[napi(getter)]
    pub fn host_path(&self) -> Result<String> {
        self.with_mount(|m| m.host_path().display().to_string())
    }

    /// The access mode: `"read-only"`, `"read-write"`, or `"overlay"`.
    #[napi(getter)]
    pub fn mode(&self) -> Result<String> {
        self.with_mount(|m| m.mode().as_str().to_owned())
    }

    /// The optional write bytes limit, or `null` if unlimited.
    #[napi(getter)]
    pub fn write_bytes_limit(&self) -> Result<Option<i64>> {
        #[expect(clippy::cast_possible_wrap)]
        self.with_mount(|m| m.write_bytes_limit().map(|v| v as i64))
    }

    /// Returns a string representation of this mount directory.
    ///
    /// # Panics
    ///
    /// Panics if the internal mutex is poisoned.
    #[napi]
    #[must_use]
    pub fn repr(&self) -> String {
        let guard = self.shared.lock().unwrap();
        match guard.as_ref() {
            Some(mount) => format!(
                "MountDirectory('{}', '{}', '{}')",
                mount.virtual_path(),
                mount.host_path().display(),
                mount.mode().as_str()
            ),
            None => "MountDirectory(<in use>)".to_owned(),
        }
    }
}

impl MountDirectory {
    /// Accesses the inner mount, returning an error if it's currently taken for a run.
    fn with_mount<T>(&self, f: impl FnOnce(&Mount) -> T) -> Result<T> {
        let guard = self.shared.lock().unwrap();
        guard.as_ref().map(f).ok_or_else(|| {
            Error::new(
                Status::GenericFailure,
                "mount directory is currently in use by a running Monty instance",
            )
        })
    }
}

// =============================================================================
// OsHandler — combines mounts for a run (internal, not exposed to JS)
// =============================================================================

/// Internal mount handler combining filesystem mounts for a run.
///
/// Not exposed to JavaScript. Built from the `mount` parameter of
/// `Monty.run()` or `Monty.start()` via [`from_js_arg`](Self::from_js_arg).
pub(crate) struct OsHandler {
    /// Shared references to each mount's storage. The mounts are **taken** out
    /// at the start of a run and **put back** when the run completes.
    mounts: Vec<SharedMount>,
}

impl OsHandler {
    /// Builds an OsHandler from pre-extracted mounts.
    ///
    /// Returns `None` if no mounts were provided.
    pub(crate) fn from_extracted(mounts: ExtractedMounts) -> Option<Self> {
        if mounts.0.is_empty() {
            return None;
        }
        Some(Self { mounts: mounts.0 })
    }

    /// Takes all mounts out of their shared slots and assembles a [`MountTable`].
    pub(crate) fn take(&self) -> Result<MountTable> {
        MountTable::take_shared_mounts(&self.mounts).map_err(|e| Error::new(Status::GenericFailure, e))
    }

    /// Puts all mounts back into their shared slots after execution completes.
    pub(crate) fn put_back(&self, table: MountTable) {
        table.put_back_shared_mounts(&self.mounts);
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Extracts shared mounts from a JS argument: `MountDirectory | MountDirectory[]`.
///
/// Uses napi's `FromNapiRef` to extract the Rust `MountDirectory` from the JS value.
pub(crate) fn extract_mounts(arg: &Object<'_>) -> Result<ExtractedMounts> {
    let env_raw = arg.value().env;

    // Try as array first
    if arg.is_array()? {
        let length: u32 = arg.get_named_property("length")?;
        let mut mounts = Vec::with_capacity(length as usize);
        for i in 0..length {
            let item: Unknown = arg.get_element(i)?;
            // SAFETY: `env_raw` is a valid napi environment, and `item.raw()` is a valid napi
            // value obtained from the array. `from_napi_ref` checks the type tag before casting.
            let md: &MountDirectory = unsafe { MountDirectory::from_napi_ref(env_raw, item.raw()) }
                .map_err(|_| Error::new(Status::InvalidArg, "mount array items must be MountDirectory"))?;
            mounts.push(Arc::clone(&md.shared));
        }
        return Ok(ExtractedMounts(mounts));
    }

    // Try as single MountDirectory
    // SAFETY: `env_raw` is a valid napi environment, and `arg.raw()` is a valid napi
    // value from the function argument. `from_napi_ref` checks the type tag before casting.
    let md: &MountDirectory = unsafe { MountDirectory::from_napi_ref(env_raw, arg.raw()) }
        .map_err(|_| Error::new(Status::InvalidArg, "mount must be a MountDirectory or MountDirectory[]"))?;
    Ok(ExtractedMounts(vec![Arc::clone(&md.shared)]))
}
