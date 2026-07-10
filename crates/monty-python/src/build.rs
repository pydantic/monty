//! Extraction of untrusted Python arguments into owned Rust values.
//!
//! Everything here converts host-supplied Python objects (source code, type
//! stubs, REPL inputs) into the owned values that get shipped to a `monty`
//! worker subprocess, turning conversion failures (lone surrogates,
//! unconvertible values) into the matching `MontyError` subclasses rather
//! than leaking raw PyO3 errors.

use ::monty::{ExcType, MontyException, MontyObject};
use monty_proto::python::{DcRegistry, exc_py_to_monty, py_to_monty_value};
use pyo3::{
    prelude::*,
    types::{PyDict, PyString},
};

use crate::exceptions::{MontyConversionError, MontyError};

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
    // Keys and values are untrusted host input. A key problem is a
    // `MontyRuntimeError` — a non-string key (`TypeError`) or a string key that
    // fails UTF-8 conversion (the lone-surrogate `ValueError` its `extract`
    // produces). A value that fails to convert goes through
    // `MontyConversionError::value_conversion_err`: an unrepresentable *type*
    // surfaces as `MontyConversionError` (a `MontyError`), exactly as an
    // `external_lookup` value does, while a depth-limit `RuntimeError` keeps its
    // type.
    inputs
        .iter()
        .map(|(key, value)| {
            let py = key.py();
            let Ok(key_str) = key.cast::<PyString>() else {
                let exc = MontyException::new(ExcType::TypeError, Some("inputs keys must be str".to_string()));
                return Err(MontyError::new_err(py, exc));
            };
            let name = key_str
                .extract::<String>()
                .map_err(|e| MontyError::new_err(py, exc_py_to_monty(py, &e)))?;
            let obj = py_to_monty_value(&value, dc_registry)
                .map_err(|e| MontyConversionError::value_conversion_err(py, e))?;
            Ok((name, obj))
        })
        .collect::<PyResult<_>>()
}
