//! Python bindings for filesystem mount configuration.
//!
//! Exposes [`PyMountDir`] (a single mount point with shared overlay state)
//! and [`OsHandler`] (a collection of mounts with optional fallback callback).
//! Filesystem operations are handled entirely in Rust via the core
//! [`monty::fs::MountTable`], with no Python round-trip.
//!
//! # Take/put pattern
//!
//! [`PyMountDir`] owns its [`Mount`] behind `Arc<Mutex<Option<Mount>>>`.
//! The Python pool sends mount *configuration* to subprocess workers for each
//! feed. Overlay state is therefore per feed in the worker; the host-side
//! `MountDir` is reusable configuration, not a live overlay store.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use monty::fs::{Mount, MountMode};
use monty_proto::python::exc_monty_to_py;
use pyo3::{exceptions::PyValueError, prelude::*};

/// Shared storage for a [`Mount`] that can be temporarily taken for execution.
pub(crate) type SharedMount = Arc<Mutex<Option<Mount>>>;

// =============================================================================
// MountDir — owns a shared Mount
// =============================================================================

/// A single mount point mapping a virtual path to a host directory.
///
/// Owns the underlying [`Mount`] via shared storage. In subprocess execution,
/// passing this to multiple feeds reuses the configuration; `'overlay'` writes
/// live only for the feed currently running in the worker.
///
/// The `mode` controls sandbox access:
/// - `'read-only'` — sandbox can read but not write
/// - `'read-write'` — sandbox can read and write real host files
/// - `'overlay'` — reads fall through to host; writes are captured in memory
#[pyclass(name = "MountDir")]
pub struct PyMountDir {
    /// Shared mount storage. `None` while a run is in progress.
    pub(crate) shared: SharedMount,
}

#[pymethods]
impl PyMountDir {
    /// Creates a new mount directory.
    ///
    /// # Arguments
    /// * `virtual_path` — absolute virtual path prefix (e.g. `"/data"`)
    /// * `host_path` — path to the real host directory
    /// * `mode` — access mode: `"read-only"`, `"read-write"`, or `"overlay"` (default)
    ///
    /// # Raises
    /// `ValueError` if `mode` is not one of the allowed values, the virtual path
    /// is not absolute, or the host path doesn't exist or isn't a directory.
    #[new]
    #[pyo3(signature = (virtual_path, host_path, *, mode = "overlay", write_bytes_limit = None))]
    #[expect(clippy::needless_pass_by_value)] // PyO3 requires owned PathBuf for conversion from Python str/Path
    fn new(
        py: Python<'_>,
        virtual_path: &str,
        host_path: PathBuf,
        mode: &str,
        write_bytes_limit: Option<u64>,
    ) -> PyResult<Self> {
        let mount_mode = MountMode::from_mode_str(mode).map_err(PyValueError::new_err)?;
        let mount = Mount::new(virtual_path, &host_path, mount_mode, write_bytes_limit)
            .map_err(|e| exc_monty_to_py(py, e.into_exception()))?;
        Ok(Self {
            shared: Arc::new(Mutex::new(Some(mount))),
        })
    }

    /// The normalized virtual path prefix inside the sandbox.
    #[getter]
    fn virtual_path(&self) -> PyResult<String> {
        self.with_mount(|m| m.virtual_path().to_owned())
    }

    /// The canonical host directory path.
    #[getter]
    fn host_path(&self) -> PyResult<String> {
        self.with_mount(|m| m.host_path().display().to_string())
    }

    /// The access mode: `"read-only"`, `"read-write"`, or `"overlay"`.
    #[getter]
    fn mode(&self) -> PyResult<String> {
        self.with_mount(|m| m.mode().as_str().to_owned())
    }

    /// The optional write bytes limit, or `None` if unlimited.
    #[getter]
    fn write_bytes_limit(&self) -> PyResult<Option<u64>> {
        self.with_mount(Mount::write_bytes_limit)
    }

    fn __repr__(&self) -> String {
        let guard = self.shared.lock().unwrap();
        match guard.as_ref() {
            Some(mount) => format!(
                "MountDir('{}', '{}', '{}')",
                mount.virtual_path(),
                mount.host_path().display(),
                mount.mode().as_str()
            ),
            None => "MountDir(<in use>)".to_owned(),
        }
    }
}

impl PyMountDir {
    /// Extracts `(virtual_path, host_path, mode, write_bytes_limit)` for use
    /// by the worker pools, which send the mount *configuration* to a worker
    /// process instead of using the `Mount` in-process.
    pub(crate) fn spec_parts(&self) -> PyResult<(String, PathBuf, &'static str, Option<u64>)> {
        self.with_mount(|m| {
            (
                m.virtual_path().to_owned(),
                m.host_path().to_path_buf(),
                m.mode().as_str(),
                m.write_bytes_limit(),
            )
        })
    }

    /// Accesses the inner mount, returning an error if it's currently taken for a run.
    fn with_mount<T>(&self, f: impl FnOnce(&Mount) -> T) -> PyResult<T> {
        let guard = self.shared.lock().unwrap();
        guard
            .as_ref()
            .map(f)
            .ok_or_else(|| PyValueError::new_err("mount directory is currently in use by a running Monty instance"))
    }
}
