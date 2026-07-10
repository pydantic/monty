//! Conversion between Monty's `MontyException`/`ExcType` and native Python
//! exceptions, in both directions.
//!
//! `exc_monty_to_py` rebuilds the closest native exception for a sandbox error
//! surfacing to the host; `exc_py_to_monty`/`exc_to_monty_object` classify a
//! host exception flowing into the sandbox (external-function errors, resumed
//! snapshots). The Python-facing `MontyError` class hierarchy stays in
//! `pydantic-monty` — this module only maps values.

use monty::{ExcData, ExcType, MontyException, MontyObject, UnicodeErrorObject};
use pyo3::{
    PyTypeCheck,
    exceptions::{self},
    prelude::*,
    sync::PyOnceLock,
    types::{PyBytes, PyString},
};

use super::dataclass::get_frozen_instance_error;

/// Converts Monty's `MontyException` to the matching Python exception value.
/// Traceback info is folded into the message, since PyO3 doesn't expose direct
/// traceback manipulation.
#[must_use]
pub fn exc_monty_to_py(py: Python<'_>, mut exc: MontyException) -> PyErr {
    let exc_type = exc.exc_type();
    let exc_data = exc.take_data();
    let msg = exc.into_message().unwrap_or_default();

    match exc_type {
        ExcType::Exception => exceptions::PyException::new_err(msg),
        ExcType::BaseException => exceptions::PyBaseException::new_err(msg),
        ExcType::SystemExit => exceptions::PySystemExit::new_err(msg),
        ExcType::KeyboardInterrupt => exceptions::PyKeyboardInterrupt::new_err(msg),
        ExcType::ArithmeticError => exceptions::PyArithmeticError::new_err(msg),
        ExcType::OverflowError => exceptions::PyOverflowError::new_err(msg),
        ExcType::ZeroDivisionError => exceptions::PyZeroDivisionError::new_err(msg),
        ExcType::LookupError => exceptions::PyLookupError::new_err(msg),
        ExcType::IndexError => exceptions::PyIndexError::new_err(msg),
        ExcType::KeyError => exceptions::PyKeyError::new_err(msg),
        ExcType::RuntimeError => exceptions::PyRuntimeError::new_err(msg),
        ExcType::NotImplementedError => exceptions::PyNotImplementedError::new_err(msg),
        ExcType::RecursionError => exceptions::PyRecursionError::new_err(msg),
        ExcType::AssertionError => exceptions::PyAssertionError::new_err(msg),
        ExcType::AttributeError => exceptions::PyAttributeError::new_err(msg),
        ExcType::FrozenInstanceError => {
            if let Ok(exc_cls) = get_frozen_instance_error(py)
                && let Ok(exc_instance) = exc_cls.call1((PyString::new(py, &msg),))
            {
                return PyErr::from_value(exc_instance);
            }
            // if creating the right exception fails, fallback to AttributeError which it's a subclass of
            exceptions::PyAttributeError::new_err(msg)
        }
        ExcType::MemoryError => exceptions::PyMemoryError::new_err(msg),
        ExcType::NameError => exceptions::PyNameError::new_err(msg),
        ExcType::UnboundLocalError => exceptions::PyUnboundLocalError::new_err(msg),
        ExcType::StopIteration => exceptions::PyStopIteration::new_err(msg),
        ExcType::SyntaxError => exceptions::PySyntaxError::new_err(msg),
        ExcType::TimeoutError => exceptions::PyTimeoutError::new_err(msg),
        ExcType::TypeError => exceptions::PyTypeError::new_err(msg),
        ExcType::ValueError => exceptions::PyValueError::new_err(msg),
        ExcType::UnicodeDecodeError | ExcType::UnicodeEncodeError => unicode_error_to_py(py, exc_type, exc_data, msg),
        ExcType::JsonDecodeError => {
            if let Ok(json_decode_error) = get_json_decode_error(py)
                && let Ok(exc_instance) = json_decode_error.call1((PyString::new(py, &msg),))
            {
                PyErr::from_value(exc_instance)
            } else {
                exceptions::PyValueError::new_err(msg)
            }
        }
        ExcType::ImportError => exceptions::PyImportError::new_err(msg),
        ExcType::ModuleNotFoundError => exceptions::PyModuleNotFoundError::new_err(msg),
        ExcType::OSError => exceptions::PyOSError::new_err(msg),
        ExcType::FileNotFoundError => exceptions::PyFileNotFoundError::new_err(msg),
        ExcType::FileExistsError => exceptions::PyFileExistsError::new_err(msg),
        ExcType::IsADirectoryError => exceptions::PyIsADirectoryError::new_err(msg),
        ExcType::NotADirectoryError => exceptions::PyNotADirectoryError::new_err(msg),
        ExcType::PermissionError => exceptions::PyPermissionError::new_err(msg),
        ExcType::UnsupportedOperation => {
            if let Ok(exc_cls) = get_unsupported_operation(py)
                && let Ok(exc_instance) = exc_cls.call1((PyString::new(py, &msg),))
            {
                PyErr::from_value(exc_instance)
            } else {
                // Fall back to OSError — the parent we model in `is_subclass_of`.
                exceptions::PyOSError::new_err(msg)
            }
        }
        ExcType::RePatternError => {
            if let Ok(re_pattern_error) = get_re_pattern_error(py)
                && let Ok(exc_instance) = re_pattern_error.call1((PyString::new(py, &msg),))
            {
                PyErr::from_value(exc_instance)
            } else {
                exceptions::PyRuntimeError::new_err(msg)
            }
        }
    }
}

/// Builds a real `UnicodeDecodeError` / `UnicodeEncodeError` from the
/// structured fields Monty attaches to codec errors, calling CPython's
/// five-argument constructor (`encoding, object, start, end, reason`).
///
/// Falls back to a plain `ValueError` carrying the formatted message when the
/// payload is absent — an exception raised manually inside the sandbox
/// (`raise UnicodeDecodeError('msg')`), or a codec error on an object larger
/// than `UnicodeErrorData::MAX_OBJECT_LEN` — or when construction fails
/// (e.g. a decode payload carrying a `str` object). `except ValueError:`
/// catches both forms; only `isinstance` and the attributes differ.
fn unicode_error_to_py(py: Python<'_>, exc_type: ExcType, exc_data: ExcData, msg: String) -> PyErr {
    if let ExcData::Unicode(data) = exc_data {
        let exc_cls = if exc_type == ExcType::UnicodeDecodeError {
            py.get_type::<exceptions::PyUnicodeDecodeError>()
        } else {
            py.get_type::<exceptions::PyUnicodeEncodeError>()
        };
        let object = match &data.object {
            UnicodeErrorObject::Bytes(bytes) => PyBytes::new(py, bytes).into_any(),
            UnicodeErrorObject::Str(s) => PyString::new(py, s).into_any(),
        };
        if let Ok(exc_instance) = exc_cls.call1((data.encoding, object, data.start, data.end, data.reason)) {
            return PyErr::from_value(exc_instance);
        }
    }
    exceptions::PyValueError::new_err(msg)
}

/// Converts a python exception to monty.
///
/// Used when resuming execution with an exception from Python.
pub fn exc_py_to_monty(py: Python<'_>, py_err: &PyErr) -> MontyException {
    let exc = py_err.value(py);
    let exc_type = py_err_to_exc_type(exc);
    let arg = exc.str().ok().map(|s| s.to_string_lossy().into_owned());

    MontyException::new(exc_type, arg)
}

/// Converts a Python exception to Monty's `MontyObject::Exception`.
#[must_use]
pub fn exc_to_monty_object(exc: &Bound<'_, exceptions::PyBaseException>) -> MontyObject {
    let exc_type = py_err_to_exc_type(exc);
    let arg = exc.str().ok().map(|s| s.to_string_lossy().into_owned());

    MontyObject::Exception { exc_type, arg }
}

/// Maps a Python exception type to Monty's `ExcType` enum.
///
/// NOTE: order matters here as some exceptions are subclasses of others!
/// In general we group exceptions by their type hierarchy to improve performance.
fn py_err_to_exc_type(exc: &Bound<'_, exceptions::PyBaseException>) -> ExcType {
    // Exception hierarchy
    if exceptions::PyException::type_check(exc) {
        // put the most commonly used exceptions first
        if exceptions::PyTypeError::type_check(exc) {
            ExcType::TypeError
        // ValueError hierarchy (check UnicodeDecodeError/UnicodeEncodeError first as they're subclasses)
        } else if exceptions::PyValueError::type_check(exc) {
            if is_json_decode_error(exc) {
                ExcType::JsonDecodeError
            } else if exceptions::PyUnicodeDecodeError::type_check(exc) {
                ExcType::UnicodeDecodeError
            } else if exceptions::PyUnicodeEncodeError::type_check(exc) {
                ExcType::UnicodeEncodeError
            } else if is_unsupported_operation(exc) {
                // `io.UnsupportedOperation` inherits from both `OSError` and `ValueError`
                ExcType::UnsupportedOperation
            } else {
                ExcType::ValueError
            }
        } else if exceptions::PyAssertionError::type_check(exc) {
            ExcType::AssertionError
        } else if exceptions::PySyntaxError::type_check(exc) {
            ExcType::SyntaxError
        // LookupError hierarchy
        } else if exceptions::PyLookupError::type_check(exc) {
            if exceptions::PyKeyError::type_check(exc) {
                ExcType::KeyError
            } else if exceptions::PyIndexError::type_check(exc) {
                ExcType::IndexError
            } else {
                ExcType::LookupError
            }
        // ArithmeticError hierarchy
        } else if exceptions::PyArithmeticError::type_check(exc) {
            if exceptions::PyZeroDivisionError::type_check(exc) {
                ExcType::ZeroDivisionError
            } else if exceptions::PyOverflowError::type_check(exc) {
                ExcType::OverflowError
            } else {
                ExcType::ArithmeticError
            }
        // RuntimeError hierarchy
        } else if exceptions::PyRuntimeError::type_check(exc) {
            if exceptions::PyNotImplementedError::type_check(exc) {
                ExcType::NotImplementedError
            } else if exceptions::PyRecursionError::type_check(exc) {
                ExcType::RecursionError
            } else {
                ExcType::RuntimeError
            }
        // AttributeError hierarchy
        } else if exceptions::PyAttributeError::type_check(exc) {
            if is_frozen_instance_error(exc) {
                ExcType::FrozenInstanceError
            } else {
                ExcType::AttributeError
            }
        // NameError hierarchy (check UnboundLocalError first as it's a subclass)
        } else if exceptions::PyNameError::type_check(exc) {
            if exceptions::PyUnboundLocalError::type_check(exc) {
                ExcType::UnboundLocalError
            } else {
                ExcType::NameError
            }
        // `io.UnsupportedOperation` inherits from `OSError` but is covered above
        } else if exceptions::PyOSError::type_check(exc) {
            if exceptions::PyFileNotFoundError::type_check(exc) {
                ExcType::FileNotFoundError
            } else if exceptions::PyFileExistsError::type_check(exc) {
                ExcType::FileExistsError
            } else if exceptions::PyIsADirectoryError::type_check(exc) {
                ExcType::IsADirectoryError
            } else if exceptions::PyNotADirectoryError::type_check(exc) {
                ExcType::NotADirectoryError
            } else if exceptions::PyPermissionError::type_check(exc) {
                ExcType::PermissionError
            } else {
                ExcType::OSError
            }
        // other standalone exception types
        } else if exceptions::PyTimeoutError::type_check(exc) {
            ExcType::TimeoutError
        } else if exceptions::PyMemoryError::type_check(exc) {
            ExcType::MemoryError
        } else {
            ExcType::Exception
        }
    // BaseException direct subclasses
    } else if exceptions::PySystemExit::type_check(exc) {
        ExcType::SystemExit
    } else if exceptions::PyKeyboardInterrupt::type_check(exc) {
        ExcType::KeyboardInterrupt
    // Catch-all for BaseException
    } else {
        ExcType::BaseException
    }
}

/// Checks if an exception is a `dataclasses.FrozenInstanceError` (not a built-in
/// PyO3 type, so this isinstance-checks against the imported class).
fn is_frozen_instance_error(exc: &Bound<'_, exceptions::PyBaseException>) -> bool {
    if let Ok(frozen_error_cls) = get_frozen_instance_error(exc.py()) {
        exc.is_instance(frozen_error_cls).unwrap_or(false)
    } else {
        false
    }
}

/// Checks if an exception is a `json.JSONDecodeError` (a stdlib class, not a
/// PyO3 built-in, so looked up lazily and cached).
fn is_json_decode_error(exc: &Bound<'_, exceptions::PyBaseException>) -> bool {
    if let Ok(json_decode_error_cls) = get_json_decode_error(exc.py()) {
        exc.is_instance(json_decode_error_cls).unwrap_or(false)
    } else {
        false
    }
}

/// Returns the cached `re.PatternError` class (named `re.error` before 3.13).
///
/// Runtime version check (not `cfg!(Py_3_13)`): this crate has no
/// pyo3-build-config build script, so the version cfgs don't exist.
fn get_re_pattern_error(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static RE_PATTERN_ERROR: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    if py.version_info() >= (3, 13) {
        RE_PATTERN_ERROR.import(py, "re", "PatternError")
    } else {
        RE_PATTERN_ERROR.import(py, "re", "error")
    }
}

/// Returns the cached `json.JSONDecodeError` class.
///
/// This avoids repeated imports while still using the stdlib-defined subclass
/// of `ValueError` rather than fabricating a plain `ValueError`.
fn get_json_decode_error(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static JSON_DECODE_ERROR: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
    JSON_DECODE_ERROR.import(py, "json", "JSONDecodeError")
}

/// Returns the cached `io.UnsupportedOperation` class.
///
/// Lives in Python's standard library (not in PyO3's built-in wrappers) and
/// is a subclass of both `OSError` and `ValueError` in CPython. Monty raises
/// the real CPython class here so user code can `isinstance(e,
/// io.UnsupportedOperation)`; both parents are modelled by
/// [`ExcType::is_subclass_of`], so `except OSError:` and `except ValueError:`
/// catch it just like in CPython.
fn get_unsupported_operation(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static UNSUPPORTED_OPERATION: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
    UNSUPPORTED_OPERATION.import(py, "io", "UnsupportedOperation")
}

/// Checks if an exception is an instance of `io.UnsupportedOperation`.
fn is_unsupported_operation(exc: &Bound<'_, exceptions::PyBaseException>) -> bool {
    if let Ok(cls) = get_unsupported_operation(exc.py()) {
        exc.is_instance(cls).unwrap_or(false)
    } else {
        false
    }
}
