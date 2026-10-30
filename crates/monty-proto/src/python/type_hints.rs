//! Host answers to the sandbox's `typing.get_type_hints()` OS call.
//!
//! Hints are read only when sandbox code asks for them, never on a value's
//! crossing. Each becomes a builtin type the sandbox models or its text: the
//! general encoder would turn a host class into a fake function and reject
//! `int | None`.

use monty_types::{MontyObject, MontyUuid};
use pyo3::{
    intern,
    prelude::*,
    sync::PyOnceLock,
    types::{PyDict, PyString, PyType},
};

use super::{class_instance::InstanceStore, convert::py_type_object_to_monty};

/// The hints of a host function: `typing.get_type_hints(obj)`, falling back to
/// `obj.__annotations__` (an unresolvable forward reference makes the former
/// raise), then to `{}`.
pub fn callable_type_hints(obj: &Bound<'_, PyAny>) -> MontyObject {
    static GET_TYPE_HINTS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    let py = obj.py();
    let hints = GET_TYPE_HINTS
        .import(py, "typing", "get_type_hints")
        .and_then(|get_type_hints| get_type_hints.call1((obj,)))
        .or_else(|_| obj.getattr(intern!(py, "__annotations__")))
        .and_then(|hints| Ok(hints.cast_into::<PyDict>()?));
    hints.map_or_else(|_| MontyObject::dict([]), |hints| hints_to_monty(&hints))
}

/// The hints of the host class registered under `id`, from its `ClassType`
/// wrapper's `get_type_hints()`; `{}` when no class is registered there.
pub fn class_type_hints(py: Python<'_>, store: &InstanceStore, id: &MontyUuid) -> PyResult<MontyObject> {
    let Some(wrapper) = store.get(py, id)? else {
        return Ok(MontyObject::dict([]));
    };
    let hints = wrapper
        .bind(py)
        .call_method0(intern!(py, "get_type_hints"))?
        .cast_into::<PyDict>()?;
    Ok(hints_to_monty(&hints))
}

/// Converts a hints dict, leaving out entries whose name is not a `str` or
/// whose hint cannot be read, rather than failing the whole answer.
fn hints_to_monty(hints: &Bound<'_, PyDict>) -> MontyObject {
    MontyObject::dict(hints.iter().filter_map(|(name, hint)| {
        let name = name.extract::<String>().ok()?;
        Some((MontyObject::string(name), hint_value(&hint).ok()?))
    }))
}

/// One hint: `None` stands for `NoneType`, as in a hint; a builtin type the
/// sandbox models crosses as itself, any other class as its dotted name and
/// anything else (aliases, unions) as its `repr()`.
fn hint_value(hint: &Bound<'_, PyAny>) -> PyResult<MontyObject> {
    let hint = if hint.is_none() {
        hint.get_type().into_any()
    } else {
        hint.clone()
    };
    if let Ok(text) = hint.cast::<PyString>() {
        Ok(MontyObject::string(text.to_str()?))
    } else if let Ok(ty) = hint.cast::<PyType>() {
        if let Some(t) = py_type_object_to_monty(ty)? {
            Ok(MontyObject::type_object(t))
        } else {
            let module: String = ty.module()?.extract()?;
            let name = ty.qualname()?.to_string();
            Ok(MontyObject::string(if module == "builtins" {
                name
            } else {
                format!("{module}.{name}")
            }))
        }
    } else {
        Ok(MontyObject::string(hint.repr()?.to_string()))
    }
}
