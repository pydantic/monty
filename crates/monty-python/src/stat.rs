//! Python-exposed stat_result builder functions.
//!
//! These functions create `MontyObject::NamedTuple` values that match Python's
//! `os.stat_result` structure, for use when resuming OS calls from Python.

use monty::{dir_stat, file_stat, symlink_stat};
use pyo3::{prelude::*, types::PyDict};

use crate::convert::monty_to_py;

/// Creates a stat_result for a regular file.
///
/// # Arguments
/// * `mode` - File permissions as octal (e.g., 0o644) or full mode with file type
/// * `size` - File size in bytes
/// * `mtime` - Modification time as Unix timestamp
///
/// # Returns
/// A namedtuple-like object with stat_result fields (st_mode, st_ino, st_dev, etc.)
#[pyfunction]
#[pyo3(name = "file_stat", signature = (mode, size, mtime))]
pub fn py_file_stat(py: Python<'_>, mode: i64, size: i64, mtime: f64) -> PyResult<Py<PyAny>> {
    let stat = file_stat(mode, size, mtime);
    let empty_registry = PyDict::new(py);
    monty_to_py(py, &stat, &empty_registry)
}

/// Creates a stat_result for a directory.
///
/// # Arguments
/// * `mode` - Directory permissions as octal (e.g., 0o755) or full mode with file type
/// * `mtime` - Modification time as Unix timestamp
///
/// # Returns
/// A namedtuple-like object with stat_result fields
#[pyfunction]
#[pyo3(name = "dir_stat", signature = (mode, mtime))]
pub fn py_dir_stat(py: Python<'_>, mode: i64, mtime: f64) -> PyResult<Py<PyAny>> {
    let stat = dir_stat(mode, mtime);
    let empty_registry = PyDict::new(py);
    monty_to_py(py, &stat, &empty_registry)
}

/// Creates a stat_result for a symbolic link.
///
/// # Arguments
/// * `mode` - Symlink permissions as octal (e.g., 0o777) or full mode with file type
/// * `mtime` - Modification time as Unix timestamp
///
/// # Returns
/// A namedtuple-like object with stat_result fields
#[pyfunction]
#[pyo3(name = "symlink_stat", signature = (mode, mtime))]
pub fn py_symlink_stat(py: Python<'_>, mode: i64, mtime: f64) -> PyResult<Py<PyAny>> {
    let stat = symlink_stat(mode, mtime);
    let empty_registry = PyDict::new(py);
    monty_to_py(py, &stat, &empty_registry)
}
