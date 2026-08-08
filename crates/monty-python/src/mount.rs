//! Python bindings for filesystem mount configuration.
//!
//! [`PyMountDir`] stores immutable configuration for one mount point. Each
//! feed copies that configuration into a fresh parent-side mount table, so
//! overlay state lasts only for that feed and host paths never reach workers.

use std::path::PathBuf;

use monty_fs::{MountMode, MountRoot};
use monty_pool::{MountSpec, MountSpecMode};
use monty_proto::python::exc_monty_to_py;
use pyo3::{exceptions::PyValueError, prelude::*, types::PyTuple};

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
    /// Validated configuration copied into each feed's mount table. `None`
    /// once closed — the open directory is released, so nothing can mount it.
    spec: Option<MountSpec>,
    /// What the getters report, kept so they still answer after `close()`.
    label: MountLabel,
}

/// The parts of a mount that outlive its descriptor.
struct MountLabel {
    virtual_path: String,
    host_path: String,
    mode: MountSpecMode,
    write_bytes_limit: Option<u64>,
    memory_usage_limit: u64,
}

#[pymethods]
impl PyMountDir {
    /// Creates a new mount directory. All arguments are keyword-only: mount
    /// tools disagree on host-first (docker `-v`) vs virtual-first (nginx
    /// `alias`) ordering, so requiring names removes the ambiguity.
    ///
    /// # Arguments
    /// * `host_path` — path to the real host directory
    /// * `virtual_path` — absolute virtual path prefix (e.g. `"/data"`)
    /// * `mode` — access mode: `"read-only"`, `"read-write"`, or `"overlay"` (default)
    ///
    /// # Raises
    /// `ValueError` if `mode` is not one of the allowed values, the virtual path
    /// is not absolute, or the host path doesn't exist or isn't a directory.
    #[new]
    #[pyo3(signature = (
        *,
        host_path,
        virtual_path,
        mode = "overlay",
        // must stay a literal mirroring monty_fs::DEFAULT_MEMORY_USAGE_LIMIT: a
        // const default renders as `...` in the text signature, breaking stubtest
        write_bytes_limit = None,
        memory_usage_limit = 100_000_000,
    ))]
    #[expect(clippy::needless_pass_by_value)] // PyO3 requires owned PathBuf for conversion from Python str/Path
    fn new(
        py: Python<'_>,
        host_path: PathBuf,
        virtual_path: &str,
        mode: &str,
        write_bytes_limit: Option<u64>,
        memory_usage_limit: u64,
    ) -> PyResult<Self> {
        let mount_mode = MountMode::from_mode_str(mode).map_err(PyValueError::new_err)?;
        // Held for this object's lifetime: later feeds mount the descriptor
        // rather than re-resolving `host_path`, which the sandbox can redirect.
        let root = MountRoot::open(virtual_path, &host_path).map_err(|e| exc_monty_to_py(py, e.into_exception()))?;
        let mut spec = MountSpec::from_root(
            root,
            match mount_mode {
                MountMode::ReadOnly => MountSpecMode::ReadOnly,
                MountMode::ReadWrite => MountSpecMode::ReadWrite,
                MountMode::OverlayMemory(_) => MountSpecMode::Overlay,
            },
        );
        spec.write_bytes_limit = write_bytes_limit;
        spec.memory_usage_limit = memory_usage_limit;
        Ok(Self {
            label: MountLabel {
                virtual_path: spec.virtual_path().to_owned(),
                host_path: spec.host_path().display().to_string(),
                mode: spec.mode,
                write_bytes_limit,
                memory_usage_limit,
            },
            spec: Some(spec),
        })
    }

    /// Releases the open host directory. Later feeds using this mount raise
    /// `ValueError`; the attributes below keep answering. Idempotent.
    ///
    /// Only Windows needs this: it refuses to rename or delete a directory
    /// while a handle to it is open, so a mount left open blocks the host from
    /// touching it. A feed already running keeps its own reference.
    fn close(&mut self) {
        self.spec = None;
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    #[pyo3(signature = (*_args))]
    fn __exit__(&mut self, _args: &Bound<'_, PyTuple>) -> bool {
        self.close();
        false
    }

    /// The canonical host directory path.
    #[getter]
    fn host_path(&self) -> &str {
        &self.label.host_path
    }

    /// The normalized virtual path prefix inside the sandbox.
    #[getter]
    fn virtual_path(&self) -> &str {
        &self.label.virtual_path
    }

    /// The access mode: `"read-only"`, `"read-write"`, or `"overlay"`.
    #[getter]
    fn mode(&self) -> &'static str {
        mount_mode_name(self.label.mode)
    }

    /// The optional write bytes limit, or `None` if unlimited.
    #[getter]
    fn write_bytes_limit(&self) -> Option<u64> {
        self.label.write_bytes_limit
    }

    /// The aggregate memory budget for this mount.
    #[getter]
    fn memory_usage_limit(&self) -> u64 {
        self.label.memory_usage_limit
    }

    fn __repr__(&self) -> String {
        format!(
            "MountDir(host_path='{}', virtual_path='{}', mode='{}')",
            self.label.host_path,
            self.label.virtual_path,
            mount_mode_name(self.label.mode)
        )
    }
}

impl PyMountDir {
    /// Copies the validated configuration for a new parent-side mount table.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` if the mount has been closed.
    pub(crate) fn spec(&self) -> PyResult<MountSpec> {
        self.spec
            .clone()
            .ok_or_else(|| PyValueError::new_err(format!("mount '{}' is closed", self.label.virtual_path)))
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
