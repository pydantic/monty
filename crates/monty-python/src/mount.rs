//! Python bindings for filesystem mount configuration.
//!
//! [`PyMountDir`] stores immutable configuration for one mount point. Each
//! feed copies that configuration into a fresh parent-side mount table, so
//! overlay state lasts only for that feed and host paths never reach workers.

use std::path::PathBuf;

use monty_fs::{Mount, MountMode};
use monty_pool::{MountSpec, MountSpecMode};
use monty_proto::python::exc_monty_to_py;
use pyo3::{exceptions::PyValueError, prelude::*};

// =============================================================================
// MountDir — immutable mount configuration
// =============================================================================

/// A single mount point mapping a virtual path to a host directory.
///
/// Passing one instance to multiple feeds reuses only its configuration;
/// `'overlay'` writes live in each feed's parent-side mount table.
/// Retained overlay data and filesystem results share a configurable memory
/// budget, which defaults to 100 MB.
///
/// The `mode` controls sandbox access:
/// - `'read-only'` — sandbox can read but not write
/// - `'read-write'` — sandbox can read and write real host files
/// - `'overlay'` — reads fall through to host; writes are captured in memory
#[pyclass(name = "MountDir")]
pub struct PyMountDir {
    /// Validated configuration copied into each feed's mount table.
    spec: MountSpec,
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
    #[pyo3(signature = (
        virtual_path,
        host_path,
        *,
        mode = "overlay",
        // must stay a literal mirroring monty_fs::DEFAULT_MEMORY_USAGE_LIMIT: a
        // const default renders as `...` in the text signature, breaking stubtest
        write_bytes_limit = None,
        memory_usage_limit = 100_000_000,
    ))]
    #[expect(clippy::needless_pass_by_value)] // PyO3 requires owned PathBuf for conversion from Python str/Path
    fn new(
        py: Python<'_>,
        virtual_path: &str,
        host_path: PathBuf,
        mode: &str,
        write_bytes_limit: Option<u64>,
        memory_usage_limit: u64,
    ) -> PyResult<Self> {
        let mount_mode = MountMode::from_mode_str(mode).map_err(PyValueError::new_err)?;
        let mount = Mount::new(virtual_path, &host_path, mount_mode, write_bytes_limit)
            .map_err(|e| exc_monty_to_py(py, e.into_exception()))?;
        Ok(Self {
            spec: MountSpec {
                virtual_path: mount.virtual_path().to_owned(),
                host_path: mount.host_path().to_path_buf(),
                mode: match mount.mode() {
                    MountMode::ReadOnly => MountSpecMode::ReadOnly,
                    MountMode::ReadWrite => MountSpecMode::ReadWrite,
                    MountMode::OverlayMemory(_) => MountSpecMode::Overlay,
                },
                write_bytes_limit: mount.write_bytes_limit(),
                memory_usage_limit,
            },
        })
    }

    /// The normalized virtual path prefix inside the sandbox.
    #[getter]
    fn virtual_path(&self) -> String {
        self.spec.virtual_path.clone()
    }

    /// The canonical host directory path.
    #[getter]
    fn host_path(&self) -> String {
        self.spec.host_path.display().to_string()
    }

    /// The access mode: `"read-only"`, `"read-write"`, or `"overlay"`.
    #[getter]
    fn mode(&self) -> &'static str {
        mount_mode_name(self.spec.mode)
    }

    /// The optional write bytes limit, or `None` if unlimited.
    #[getter]
    fn write_bytes_limit(&self) -> Option<u64> {
        self.spec.write_bytes_limit
    }

    /// The aggregate memory budget for this mount.
    #[getter]
    fn memory_usage_limit(&self) -> u64 {
        self.spec.memory_usage_limit
    }

    fn __repr__(&self) -> String {
        format!(
            "MountDir('{}', '{}', '{}')",
            self.spec.virtual_path,
            self.spec.host_path.display(),
            mount_mode_name(self.spec.mode)
        )
    }
}

impl PyMountDir {
    /// Copies the validated configuration for a new parent-side mount table.
    pub(crate) fn spec(&self) -> MountSpec {
        self.spec.clone()
    }
}

/// Returns the Python spelling of a pool mount mode.
fn mount_mode_name(mode: MountSpecMode) -> &'static str {
    match mode {
        MountSpecMode::ReadOnly => "read-only",
        MountSpecMode::ReadWrite => "read-write",
        MountSpecMode::Overlay => "overlay",
    }
}
