//! Exception mapping between Monty and Python.
//!
//! Converts Monty's `MontyException` and `ExcType` to PyO3's `PyErr`
//! so that Python code sees native Python exceptions.

use ::monty::{ExcType, MontyException};
use pyo3::{exceptions, prelude::*, PyTypeCheck};

/// Converts Monty's `MontyException` to a Python exception.
///
/// Creates an appropriate Python exception type with the message.
/// The traceback information is included in the exception message
/// since PyO3 doesn't provide direct traceback manipulation.
pub fn exc_monty_to_py(exc: MontyException) -> PyErr {
    let exc_type = exc.exc_type();
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
        ExcType::MemoryError => exceptions::PyMemoryError::new_err(msg),
        ExcType::NameError => exceptions::PyNameError::new_err(msg),
        ExcType::SyntaxError => exceptions::PySyntaxError::new_err(msg),
        ExcType::TimeoutError => exceptions::PyTimeoutError::new_err(msg),
        ExcType::TypeError => exceptions::PyTypeError::new_err(msg),
        ExcType::ValueError => exceptions::PyValueError::new_err(msg),
    }
}

/// Converts a python exception to monty.
pub fn exc_py_to_monty(py: Python<'_>, py_err: PyErr) -> MontyException {
    let exc = py_err.value(py);
    let exc_type = py_err_to_exc_type(exc);
    let arg = exc.str().ok().map(|s| s.to_string_lossy().into_owned());

    MontyException::new(exc_type, arg)
}

/// Converts a Python exception to Monty's `MontyObject::Exception`.
pub fn exc_to_monty_object(exc: &Bound<'_, exceptions::PyBaseException>) -> ::monty::MontyObject {
    let exc_type = py_err_to_exc_type(exc);
    let arg = exc.str().ok().map(|s| s.to_string_lossy().into_owned());

    ::monty::MontyObject::Exception { exc_type, arg }
}

/// Maps a Python exception type to Monty's `ExcType` enum.
///
/// NOTE: order matters here as some exceptions are subclasses of others!
/// More specific exceptions must be checked before their parent classes.
fn py_err_to_exc_type(exc: &Bound<'_, exceptions::PyBaseException>) -> ExcType {
    // LookupError subclasses (check specific types before parent)
    if exceptions::PyKeyError::type_check(exc) {
        ExcType::KeyError
    } else if exceptions::PyIndexError::type_check(exc) {
        ExcType::IndexError
    // ArithmeticError subclasses
    } else if exceptions::PyArithmeticError::type_check(exc) {
        ExcType::ZeroDivisionError
    } else if exceptions::PyAssertionError::type_check(exc) {
        ExcType::AssertionError
    } else if exceptions::PyAttributeError::type_check(exc) {
        ExcType::AttributeError
    } else if exceptions::PyMemoryError::type_check(exc) {
        ExcType::MemoryError
    } else if exceptions::PyNameError::type_check(exc) {
        ExcType::NameError
    } else if exceptions::PySyntaxError::type_check(exc) {
        ExcType::SyntaxError
    } else if exceptions::PyTimeoutError::type_check(exc) {
        ExcType::TimeoutError
    } else if exceptions::PyTypeError::type_check(exc) {
        ExcType::TypeError
    } else if exceptions::PyValueError::type_check(exc) {
        ExcType::ValueError
    } else if exceptions::PyRuntimeError::type_check(exc) {
        ExcType::RuntimeError
    } else if exceptions::PySystemError::type_check(exc) {
        ExcType::SystemExit
    } else if exceptions::PyKeyboardInterrupt::type_check(exc) {
        ExcType::KeyboardInterrupt
    } else if exceptions::PyException::type_check(exc) {
        ExcType::Exception
    } else {
        ExcType::BaseException
    }
}
