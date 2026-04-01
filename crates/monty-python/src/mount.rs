//! Python bindings for filesystem mount configuration.
//!
//! Exposes [`PyMountDirectory`] (a single mount point with shared overlay state)
//! and [`OsHandler`] (a collection of mounts with optional fallback callback).
//! Filesystem operations are handled entirely in Rust via the core
//! [`monty::fs::MountTable`], with no Python round-trip.
//!
//! # Take/put pattern
//!
//! [`PyMountDirectory`] owns its [`Mount`] behind `Arc<Mutex<Option<Mount>>>`.
//! When `Monty.run()` starts, each mount is **taken** out of its slot (zero-cost
//! `Option::take`), moved into a plain [`MountTable`], and execution proceeds
//! with no locking overhead. When the run finishes, each mount is **put back**.
//! This gives us shared ownership for Python while keeping the hot path lock-free.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use monty::fs::{Mount, MountMode, MountTable};
use pyo3::{
    exceptions::{PyTypeError, PyValueError},
    prelude::*,
    types::PyList,
};

use crate::exceptions::exc_monty_to_py;

/// Shared storage for a [`Mount`] that can be temporarily taken for execution.
pub(crate) type SharedMount = Arc<Mutex<Option<Mount>>>;

// =============================================================================
// MountDirectory — owns a shared Mount
// =============================================================================

/// A single mount point mapping a virtual path to a host directory.
///
/// Owns the underlying [`Mount`] (including overlay state for `'overlay'` mode)
/// via shared storage, so passing this to multiple [`OsHandler`]s or reusing
/// it across `Monty.run()` calls preserves the same overlay data.
///
/// The `mode` controls sandbox access:
/// - `'read-only'` — sandbox can read but not write
/// - `'read-write'` — sandbox can read and write real host files
/// - `'overlay'` — reads fall through to host; writes are captured in memory
#[pyclass(name = "MountDirectory")]
pub struct PyMountDirectory {
    /// Shared mount storage. `None` while a run is in progress.
    pub(crate) shared: SharedMount,
}

#[pymethods]
impl PyMountDirectory {
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
                "MountDirectory('{}', '{}', '{}')",
                mount.virtual_path(),
                mount.host_path().display(),
                mount.mode().as_str()
            ),
            None => "MountDirectory(<in use>)".to_owned(),
        }
    }
}

impl PyMountDirectory {
    /// Accesses the inner mount, returning an error if it's currently taken for a run.
    fn with_mount<T>(&self, f: impl FnOnce(&Mount) -> T) -> PyResult<T> {
        let guard = self.shared.lock().unwrap();
        guard
            .as_ref()
            .map(f)
            .ok_or_else(|| PyValueError::new_err("mount directory is currently in use by a running Monty instance"))
    }
}

// =============================================================================
// Internal mount table — combines mount + os parameters for a run
// =============================================================================

/// Internal mount table combining filesystem mounts with an optional OS callback.
///
/// Not exposed to Python. Built from the `mount` and `os` parameters of
/// `Monty.run()` via [`from_run_args`](Self::from_run_args).
pub(crate) struct OsHandler {
    /// Shared references to each mount's storage. The mounts are **taken** out
    /// at the start of a run and **put back** when the run completes.
    mounts: Vec<SharedMount>,
    /// Optional Python callable for non-filesystem OS operations.
    pub(crate) fallback: Option<Py<PyAny>>,
}

impl OsHandler {
    /// Builds an internal mount table from the `mount` and `os` parameters
    /// of `Monty.run()`.
    ///
    /// - `mount`: `MountDirectory | list[MountDirectory] | None`
    /// - `os`: `Callable | None` — fallback for non-filesystem OS operations
    ///
    /// Returns `None` if both are `None`, meaning no OS handling is configured.
    pub(crate) fn from_run_args(
        _py: Python<'_>,
        mount: Option<&Bound<'_, PyAny>>,
        os: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Option<Self>> {
        let mounts = match mount {
            Some(arg) => extract_mounts(arg)?,
            None => vec![],
        };

        let fallback = match os {
            Some(cb) => {
                if !cb.is_callable() {
                    return Err(PyTypeError::new_err(format!(
                        "os must be callable, got '{}'",
                        cb.get_type().name()?
                    )));
                }
                Some(cb.clone().unbind())
            }
            None => None,
        };

        if mounts.is_empty() && fallback.is_none() {
            return Ok(None);
        }

        // For backwards compatibility: if only `os` is provided (no mounts),
        // the callable handles all OS operations including filesystem ops.
        Ok(Some(Self { mounts, fallback }))
    }

    /// Takes all mounts out of their shared slots and assembles a [`MountTable`].
    pub(crate) fn take(&self) -> PyResult<MountTable> {
        MountTable::take_shared_mounts(&self.mounts).map_err(PyValueError::new_err)
    }

    /// Puts all mounts back into their shared slots after execution completes.
    pub(crate) fn put_back(&self, table: MountTable) {
        table.put_back_shared_mounts(&self.mounts);
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Extracts shared mounts from `mount` argument: `MountDirectory | list[MountDirectory]`.
fn extract_mounts(arg: &Bound<'_, PyAny>) -> PyResult<Vec<SharedMount>> {
    if let Ok(md) = arg.cast::<PyMountDirectory>() {
        // Single MountDirectory
        Ok(vec![Arc::clone(&md.borrow().shared)])
    } else if let Ok(list) = arg.cast::<PyList>() {
        // List of MountDirectory
        let mut mounts = Vec::with_capacity(list.len());
        for item in list.iter() {
            let md: PyRef<'_, PyMountDirectory> = item.extract().map_err(|_| {
                if let Ok(t) = item.get_type().name() {
                    PyTypeError::new_err(format!("mount list items must be MountDirectory, got '{t}'"))
                } else {
                    PyTypeError::new_err("mount list items must be MountDirectory")
                }
            })?;
            mounts.push(Arc::clone(&md.shared));
        }
        Ok(mounts)
    } else if let Ok(t) = arg.get_type().name() {
        Err(PyTypeError::new_err(format!(
            "mount must be a MountDirectory or list[MountDirectory], got '{t}'"
        )))
    } else {
        Err(PyTypeError::new_err(
            "mount must be a MountDirectory or list[MountDirectory]",
        ))
    }
}
