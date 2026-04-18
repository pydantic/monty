//! Off-GIL construction of [`PyMonty`] instances.
//!
//! This module hosts everything that can run without holding the GIL when a
//! `Monty` is being parsed and (optionally) type-checked: the Rust-only error
//! types ([`TypeCheckErr`], [`BuildErr`]), the type-check entry points
//! ([`run_type_check_off_gil`], [`py_type_check`]), and the [`ConstructInputs`]
//! bundle that the sync `Monty::__new__` (via `py.detach`) and the async
//! `Monty.acreate` (via `tokio::task::spawn_blocking`) both consume.
//!
//! Keeping these together lets both constructors share a single code path while
//! also serving the standalone type-check entry points used elsewhere
//! (`Monty::type_check`, `MontyRepl::type_check`, `MontyRepl::feed_*`).

use ::monty::{ExcType, MontyException, MontyRun};
use monty_type_checking::{SourceFile, TypeCheckingDiagnostics, type_check};
use pyo3::{
    exceptions::{PyRuntimeError, PyTypeError},
    prelude::*,
    types::{PyList, PyString},
};

use crate::{
    dataclass::DcRegistry,
    exceptions::{MontyError, MontyTypingError},
    monty_cls::PyMonty,
};

/// The result of running [`run_type_check_off_gil`] without holding the GIL.
///
/// The two failure modes are kept separate so the GIL-side conversion can raise
/// [`MontyTypingError`] for diagnostic findings and [`PyRuntimeError`] for
/// internal type-checker failures, matching the behavior of [`py_type_check`].
pub(crate) enum TypeCheckErr {
    /// The type checker reported diagnostic errors in the user code.
    Diagnostics(TypeCheckingDiagnostics),
    /// The type checker itself failed (infrastructure error, not a user error).
    Internal(String),
}

impl TypeCheckErr {
    /// Converts this error into the appropriate Python exception.
    ///
    /// Must be called while holding the GIL. Diagnostic errors become
    /// [`MontyTypingError`]; infrastructure errors become [`PyRuntimeError`].
    pub(crate) fn into_pyerr(self, py: Python<'_>) -> PyErr {
        match self {
            Self::Diagnostics(diag) => MontyTypingError::new_err(py, diag),
            Self::Internal(msg) => PyRuntimeError::new_err(msg),
        }
    }
}

/// Runs the static type checker without touching Python — safe to call inside
/// `py.detach(...)` or a `tokio::task::spawn_blocking` worker.
///
/// This is the building block shared by [`py_type_check`] (sync GIL-aware
/// wrapper) and [`ConstructInputs::build`] (used by both the sync `__new__`
/// constructor and the async `Monty.acreate` classmethod).
pub(crate) fn run_type_check_off_gil(
    code: &str,
    script_name: &str,
    type_stubs: Option<&str>,
    stubs_name: &str,
) -> Result<(), TypeCheckErr> {
    let type_stubs = type_stubs.map(|s| SourceFile::new(s, stubs_name));
    match type_check(&SourceFile::new(code, script_name), type_stubs.as_ref()) {
        Ok(None) => Ok(()),
        Ok(Some(diag)) => Err(TypeCheckErr::Diagnostics(diag)),
        Err(msg) => Err(TypeCheckErr::Internal(msg)),
    }
}

/// GIL-aware wrapper around [`run_type_check_off_gil`] that releases the GIL
/// for the duration of the type-check work and converts errors to `PyErr`.
pub(crate) fn py_type_check(
    py: Python<'_>,
    code: &str,
    script_name: &str,
    type_stubs: Option<&str>,
    stubs_name: &str,
) -> PyResult<()> {
    py.detach(|| run_type_check_off_gil(code, script_name, type_stubs, stubs_name))
        .map_err(|e| e.into_pyerr(py))
}

/// Errors that can occur while assembling a [`PyMonty`] in [`ConstructInputs::build`].
///
/// Carries a Rust-only payload so the build can run inside `py.detach` or a
/// `spawn_blocking` worker; the GIL-side wrapper converts to the matching
/// `PyErr` afterwards via [`Self::into_pyerr`].
pub(crate) enum BuildErr {
    /// Type checking found a problem (diagnostics or infrastructure failure).
    TypeCheck(TypeCheckErr),
    /// Parsing the user code failed (raised as `MontySyntaxError` etc).
    Parse(MontyException),
}

impl BuildErr {
    /// Converts this error into the appropriate Python exception.
    ///
    /// Must be called while holding the GIL.
    pub(crate) fn into_pyerr(self, py: Python<'_>) -> PyErr {
        match self {
            Self::TypeCheck(e) => e.into_pyerr(py),
            Self::Parse(e) => MontyError::new_err(py, e),
        }
    }
}

/// All owned inputs needed to construct a [`PyMonty`].
///
/// Built on the GIL by [`ConstructInputs::from_py`] from the Python-side
/// arguments shared by `Monty::__new__` and `Monty.acreate`, then consumed
/// off-GIL via [`ConstructInputs::build`] so the heavy parse + type-check
/// work never blocks the caller.
pub(crate) struct ConstructInputs {
    code: String,
    script_name: String,
    input_names: Vec<String>,
    do_type_check: bool,
    type_check_stubs: Option<String>,
    dc_registry: DcRegistry,
}

impl ConstructInputs {
    /// Reads everything from the Python arguments while holding the GIL,
    /// returning an owned bundle that is `Send` for the async path.
    pub(crate) fn from_py(
        py: Python<'_>,
        code: &Bound<'_, PyString>,
        script_name: &str,
        inputs: Option<&Bound<'_, PyList>>,
        do_type_check: bool,
        type_check_stubs: Option<&str>,
        dataclass_registry: Option<&Bound<'_, PyList>>,
    ) -> PyResult<Self> {
        Ok(Self {
            input_names: list_str(inputs, "inputs")?,
            code: extract_source_code(py, code)?,
            script_name: script_name.to_string(),
            do_type_check,
            type_check_stubs: type_check_stubs.map(str::to_owned),
            dc_registry: DcRegistry::from_list(py, dataclass_registry)?,
        })
    }

    /// Off-GIL parse + assembly of the `PyMonty` instance. Safe to call from
    /// `py.detach` (sync `__new__`) or `tokio::task::spawn_blocking` (`acreate`).
    pub(crate) fn build(self) -> Result<PyMonty, BuildErr> {
        let Self {
            code,
            script_name,
            input_names,
            do_type_check,
            type_check_stubs,
            dc_registry,
        } = self;

        if do_type_check {
            run_type_check_off_gil(&code, &script_name, type_check_stubs.as_deref(), "type_stubs.pyi")
                .map_err(BuildErr::TypeCheck)?;
        }
        // `MontyRun::new` consumes the input-name vec, so clone for storage in
        // `PyMonty` (one cheap allocation, off-GIL).
        let runner = MontyRun::new(code, &script_name, input_names.clone()).map_err(BuildErr::Parse)?;
        Ok(PyMonty::from_parts(runner, script_name, input_names, dc_registry))
    }
}

/// Extracts Python source code from a `PyString`, converting encoding failures
/// into a `MontySyntaxError` rather than letting the raw `UnicodeEncodeError`
/// bubble up.
///
/// Python strings may contain lone surrogates (e.g. `'\ud83d'`) that cannot be
/// encoded as UTF-8. Such strings are not valid Python source, so we report
/// them as a syntax error instead of an encoding error.
pub(crate) fn extract_source_code(py: Python<'_>, code: &Bound<'_, PyString>) -> PyResult<String> {
    match code.to_str() {
        Ok(s) => Ok(s.to_owned()),
        Err(_) => Err(MontyError::new_err(
            py,
            MontyException::new(
                ExcType::SyntaxError,
                Some("source code is not valid UTF-8 (contains lone surrogates)".to_string()),
            ),
        )),
    }
}

/// Extracts a list of strings from an optional Python list argument, raising a
/// helpful `TypeError` (with the argument name) when an element is the wrong
/// type.
fn list_str(arg: Option<&Bound<'_, PyList>>, name: &str) -> PyResult<Vec<String>> {
    if let Some(names) = arg {
        names
            .iter()
            .map(|item| item.extract::<String>())
            .collect::<PyResult<Vec<_>>>()
            .map_err(|e| PyTypeError::new_err(format!("{name}: {e}")))
    } else {
        Ok(vec![])
    }
}
