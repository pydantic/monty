//! Host answers to the sandbox's `typing.get_type_hints()` OS call.
//!
//! Hints are read only when sandbox code asks for them, never on a value's
//! crossing. Each becomes a builtin type the sandbox models or its text: the
//! general encoder would turn a host class into a fake function and reject
//! `int | None`.

use monty_proto::python::{InstanceStore, is_class_type_wrapper, py_type_object_to_monty};
use monty_types::{MontyObject, MontyUuid};
use pyo3::{
    exceptions::PyTypeError,
    intern,
    prelude::*,
    sync::PyOnceLock,
    types::{PyMapping, PyString, PyType},
};

/// The hints of a host function, from `typing.get_type_hints(obj)`. Its
/// exception (`NameError` for an unresolvable forward reference) is raised in
/// the sandbox, as CPython raises it.
pub(crate) fn callable_type_hints(obj: &Bound<'_, PyAny>) -> PyResult<MontyObject> {
    static GET_TYPE_HINTS: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    let hints = GET_TYPE_HINTS
        .import(obj.py(), "typing", "get_type_hints")?
        .call1((obj,))?
        .cast_into::<PyMapping>()?;
    hints_to_monty(&hints)
}

/// The hints of the host class registered under `id`, from its `ClassType`
/// wrapper's `get_type_hints()`, which an override may answer with any mapping;
/// `{}` when nothing is registered there. The store also holds instance wrappers,
/// which an untrusted child could name: those are a miss too.
pub(crate) fn class_type_hints(py: Python<'_>, store: &InstanceStore, id: &MontyUuid) -> PyResult<MontyObject> {
    let Some(wrapper) = store.get(py, id)? else {
        return Ok(MontyObject::dict([]));
    };
    let wrapper = wrapper.bind(py);
    if !is_class_type_wrapper(wrapper)? {
        return Ok(MontyObject::dict([]));
    }
    let hints = wrapper.call_method0(intern!(py, "get_type_hints"))?;
    match hints.cast::<PyMapping>() {
        Ok(hints) => hints_to_monty(hints),
        Err(_) => Err(PyTypeError::new_err(format!(
            "ClassType.get_type_hints() must return a mapping, not {}",
            hints.get_type().name()?
        ))),
    }
}

/// Converts a hints mapping, leaving out entries whose name is not a `str` or
/// whose hint cannot be read, rather than failing the whole answer.
fn hints_to_monty(hints: &Bound<'_, PyMapping>) -> PyResult<MontyObject> {
    let items = hints.items()?;
    Ok(MontyObject::dict(items.iter().filter_map(|item| {
        let (name, hint) = item.extract::<(String, Bound<'_, PyAny>)>().ok()?;
        Some((MontyObject::string(name), hint_value(&hint).ok()?))
    })))
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
