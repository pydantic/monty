//! Extraction of untrusted Python arguments into owned Rust values.
//!
//! Everything here converts host-supplied Python objects (source code, type
//! stubs, REPL inputs) into the owned values that get shipped to a `monty`
//! worker subprocess, turning conversion failures (lone surrogates,
//! unconvertible values) into the matching `MontyError` subclasses rather
//! than leaking raw PyO3 errors.

use ::monty::{ExcType, MontyException, MontyObject};
use pyo3::{
    exceptions::PyTypeError,
    prelude::*,
    types::{PyDict, PyString},
};

use crate::{convert::py_to_monty_value, dataclass::DcRegistry, exceptions::MontyError};

/// Extracts source code, converting invalid UTF-8 (lone surrogates) into a
/// `MontySyntaxError` — text that cannot be decoded is not valid Python
/// source, so a syntax error is the honest classification.
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

/// Extracts the optional `type_check_stubs` argument, converting invalid
/// UTF-8 into a `MontySyntaxError` (same rationale as
/// [`extract_source_code`]).
pub(crate) fn extract_type_check_stubs(
    py: Python<'_>,
    type_check_stubs: Option<&Bound<'_, PyString>>,
) -> PyResult<Option<String>> {
    match type_check_stubs {
        Some(stubs) => match stubs.to_str() {
            Ok(s) => Ok(Some(s.to_owned())),
            Err(_) => Err(MontyError::new_err(
                py,
                MontyException::new(
                    ExcType::SyntaxError,
                    Some("type_check_stubs is not valid UTF-8".to_string()),
                ),
            )),
        },
        None => Ok(None),
    }
}

/// Extracts the `inputs` dict into `(name, value)` pairs for a feed.
pub(crate) fn extract_repl_inputs(
    inputs: Option<&Bound<'_, PyDict>>,
    dc_registry: &DcRegistry,
) -> PyResult<Vec<(String, MontyObject)>> {
    let Some(inputs) = inputs else {
        return Ok(vec![]);
    };
    // Values are untrusted host values, so conversion failures surface as
    // `MontyRuntimeError`. Keys are part of this host API surface and must be
    // strings; non-string keys are caller misuse, so they stay `TypeError`.
    inputs
        .iter()
        .map(|(key, value)| {
            let py = key.py();
            let name = key
                .extract::<String>()
                .map_err(|_| PyTypeError::new_err("inputs keys must be str"))?;
            let obj = py_to_monty_value(&value, dc_registry).map_err(|e| MontyError::new_err(py, e))?;
            Ok((name, obj))
        })
        .collect::<PyResult<_>>()
}
