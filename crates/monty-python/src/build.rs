//! Extraction of untrusted Python arguments into owned Rust values.
//!
//! Everything here converts host-supplied Python objects (source code, type
//! stubs, REPL inputs) into the owned values that get shipped to a `monty`
//! worker subprocess, turning conversion failures (lone surrogates,
//! unconvertible values) into the matching `MontyError` subclasses rather
//! than leaking raw PyO3 errors.

use monty_proto::python::{GraphEncoder, InstanceStore, exc_py_to_monty};
use monty_types::{ExcType, MontyException, NamedValues, StringRepr};
use pyo3::{
    exceptions::PyTypeError,
    prelude::*,
    types::{PyDict, PyMapping, PyString},
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

/// Extracts the `inputs` dict into the named values of a feed: one arena for
/// every input, so an object passed under two names is one sandbox object.
pub(crate) fn extract_repl_inputs(
    inputs: Option<&Bound<'_, PyDict>>,
    instances: &InstanceStore,
) -> PyResult<NamedValues> {
    let Some(inputs) = inputs else {
        return Ok(NamedValues::new());
    };
    let py = inputs.py();
    // Keys and values are untrusted host input. A key problem is a
    // `MontyRuntimeError` — a non-string key (`TypeError`) or a string key that
    // fails UTF-8 conversion (the lone-surrogate `ValueError` its `extract`
    // produces). A value that fails to convert goes through
    // `MontyConversionError::value_conversion_err`: an unrepresentable *type*
    // surfaces as `MontyConversionError` (a `MontyError`), exactly as an
    // `external_lookup` value does, while a cyclic value's `ValueError` keeps
    // its type.
    let mut encoder = GraphEncoder::new(py, instances);
    let mut names = Vec::with_capacity(inputs.len());
    for (key, value) in inputs.iter() {
        let Ok(key_str) = key.cast::<PyString>() else {
            let exc = MontyException::new(ExcType::TypeError, Some("inputs keys must be str".to_string()));
            return Err(MontyError::new_err(py, exc));
        };
        let name = key_str
            .extract::<String>()
            .map_err(|e| MontyError::new_err(py, exc_py_to_monty(py, &e)))?;
        let id = encoder
            .push(&value)
            .map_err(|e| MontyConversionError::value_conversion_err(py, exc_py_to_monty(py, &e)))?;
        names.push((name, id));
    }
    Ok(NamedValues {
        graph: encoder.finish(),
        names,
    })
}

/// Calls the `connect_headers` callback and extracts its `str -> str` mapping.
/// The GIL is held on the caller's task, so the callback sees the caller's
/// contextvars.
pub(crate) fn extract_connect_headers(py: Python<'_>, callback: &Py<PyAny>) -> PyResult<Vec<(String, String)>> {
    let result = callback.bind(py).call0()?;
    if let Ok(mapping) = result.cast::<PyMapping>() {
        mapping
            .items()?
            .iter()
            .map(|item| {
                let (name, value): (Bound<'_, PyAny>, Bound<'_, PyAny>) = item.extract()?;
                Ok((header_str(&name, "name")?, header_str(&value, "value")?))
            })
            .collect()
    } else {
        let t = result.get_type().name()?;
        Err(PyTypeError::new_err(format!(
            "connect_headers must return a mapping of str to str, got {}",
            StringRepr(&t.to_string_lossy())
        )))
    }
}

/// Extracts one header name or value, naming which side was not a `str`.
fn header_str(part: &Bound<'_, PyAny>, side: &str) -> PyResult<String> {
    if let Ok(s) = part.cast::<PyString>() {
        Ok(s.to_cow()?.into_owned())
    } else {
        let t = part.get_type().name()?;
        Err(PyTypeError::new_err(format!(
            "connect_headers must return a mapping of str to str, got {} header {}",
            StringRepr(&t.to_string_lossy()),
            side
        )))
    }
}
