//! `pydantic_monty.MontyNamedTuple`: the one host class every sandbox namedtuple decodes to.
//!
//! A class per sandbox type, as `collections.namedtuple` builds, would let the sandbox spend host memory outside
//! its limits (~5.5 KB per type), so instances carry their type name and fields instead.
//! The class is built here, with `type(...)` and Rust methods, rather than imported from the `pydantic_monty`
//! package: `monty-cpython` embeds an interpreter without that package and decodes through this crate too.

use pyo3::{
    exceptions::{PyAttributeError, PyTypeError, PyValueError},
    intern,
    prelude::*,
    sync::PyOnceLock,
    types::{PyDict, PyList, PySet, PyString, PyTuple, PyType},
};

use super::convert::import_builtins;

/// The class's docstring, shown by `help()` and the API docs' stub.
const DOC: &str = "A namedtuple returned by the sandbox: a tuple whose fields are also attributes.

It supports the instance API of `collections.namedtuple` (`p.x`, `_fields`, `_asdict()`, `_replace()`,
`_make()`), but every sandbox namedtuple type shares this one class, so `type(p)` is `MontyNamedTuple`
and the sandbox's type name is `p._type_name`, such as `'Point'` or `'os.stat_result'`.";

/// Returns the `MontyNamedTuple` class, building it on first use in this interpreter.
///
/// Its `__module__` is `pydantic_monty`, which re-exports it, so pickles name `pydantic_monty.MontyNamedTuple`.
pub fn monty_named_tuple_type(py: Python<'_>) -> PyResult<&Bound<'_, PyType>> {
    static CLASS: PyOnceLock<Py<PyType>> = PyOnceLock::new();

    CLASS
        .get_or_try_init(py, || build_class(py))
        .map(|class| class.bind(py))
}

/// Builds the class: `type('MontyNamedTuple', (tuple,), namespace)`.
fn build_class(py: Python<'_>) -> PyResult<Py<PyType>> {
    let namespace = PyDict::new(py);
    namespace.set_item("__module__", "pydantic_monty")?;
    namespace.set_item("__qualname__", "MontyNamedTuple")?;
    namespace.set_item("__doc__", DOC)?;
    // `type` leaves a non-function `__new__` unwrapped, and calls it with the class first
    namespace.set_item("__new__", wrap_pyfunction!(new, py)?)?;

    let methods = [
        wrap_pyfunction!(get_attr, py)?,
        wrap_pyfunction!(set_attr, py)?,
        wrap_pyfunction!(del_attr, py)?,
        wrap_pyfunction!(repr, py)?,
        wrap_pyfunction!(reduce, py)?,
        wrap_pyfunction!(get_new_args, py)?,
        wrap_pyfunction!(dir, py)?,
        wrap_pyfunction!(make, py)?,
        wrap_pyfunction!(replace, py)?,
        wrap_pyfunction!(as_dict, py)?,
    ];
    for function in methods {
        let name = function.getattr(intern!(py, "__name__"))?;
        namespace.set_item(name, Method(function.into_any().unbind()))?;
    }
    // `copy.replace` support is new in 3.13
    if py.version_info() >= (3, 13) {
        namespace.set_item("__replace__", namespace.get_item("_replace")?)?;
    }

    let property = import_builtins(py)?.getattr(py, intern!(py, "property"))?;
    namespace.set_item(
        "_field_defaults",
        property.call1(py, (wrap_pyfunction!(field_defaults, py)?,))?,
    )?;
    // tuple methods a field of the same name shadows, as on a real namedtuple
    namespace.set_item("count", property.call1(py, (wrap_pyfunction!(count, py)?,))?)?;
    namespace.set_item("index", property.call1(py, (wrap_pyfunction!(index, py)?,))?)?;

    let class = py
        .get_type::<PyType>()
        .call1(("MontyNamedTuple", (py.get_type::<PyTuple>(),), namespace))?;
    Ok(class.cast_into::<PyType>()?.unbind())
}

/// Binds a Rust function as an instance method; a plain builtin function has no `__get__`, so it would not.
///
/// Slot lookups (`repr()`, a missed attribute) go through `__get__` too, so dunders bind the same way.
#[pyclass(frozen, name = "_RustMethod", module = "pydantic_monty")]
struct Method(Py<PyAny>);

#[pymethods]
impl Method {
    fn __get__(
        &self,
        py: Python<'_>,
        instance: Option<Bound<'_, PyAny>>,
        _owner: Option<Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        match instance {
            Some(instance) => Ok(method_type(py)?.call1((self.0.clone_ref(py), instance))?.unbind()),
            // looked up on the class, the function takes the instance explicitly
            None => Ok(self.0.clone_ref(py)),
        }
    }
}

/// Returns `types.MethodType`, which binds any callable to an instance.
fn method_type(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static METHOD_TYPE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    METHOD_TYPE.import(py, "types", "MethodType")
}

/// `MontyNamedTuple(type_name, fields, values)`: a tuple of `values` carrying the type's name and field names.
#[pyfunction]
#[pyo3(name = "__new__")]
fn new<'py>(
    cls: &Bound<'py, PyType>,
    type_name: Bound<'py, PyAny>,
    fields: Bound<'py, PyAny>,
    values: Bound<'py, PyAny>,
) -> PyResult<Bound<'py, PyAny>> {
    let py = cls.py();
    let instance = py
        .get_type::<PyTuple>()
        .call_method1(intern!(py, "__new__"), (cls, values))?;
    let (length, expected) = (instance.len()?, fields.len()?);
    if length != expected {
        return Err(PyTypeError::new_err(format!(
            "Expected {expected} arguments, got {length}"
        )));
    }
    // straight into `__dict__`, past the `__setattr__` that makes instances immutable
    let dict = instance.getattr(intern!(py, "__dict__"))?;
    dict.set_item(intern!(py, "_type_name"), type_name)?;
    dict.set_item(intern!(py, "_fields"), fields)?;
    Ok(instance)
}

/// Field access: only reached when normal attribute lookup fails.
#[pyfunction]
#[pyo3(name = "__getattr__")]
fn get_attr<'py>(slf: &Bound<'py, PyAny>, name: &Bound<'py, PyString>) -> PyResult<Bound<'py, PyAny>> {
    // an instance missing its own attributes (never built by `__new__`) raises rather than recursing
    if matches!(name.to_str()?, "_type_name" | "_fields") {
        return Err(PyAttributeError::new_err(name.clone().unbind()));
    }
    match field_position(slf, name)? {
        Some(position) => slf.get_item(position),
        None => Err(PyAttributeError::new_err(format!(
            "{} object has no attribute {}",
            simple_name_repr(slf)?,
            name.repr()?
        ))),
    }
}

/// Raises: sandbox namedtuples are immutable.
#[pyfunction]
#[pyo3(name = "__setattr__")]
fn set_attr(slf: &Bound<'_, PyAny>, _name: &Bound<'_, PyAny>, _value: &Bound<'_, PyAny>) -> PyResult<()> {
    Err(immutable_error(slf))
}

/// Raises: sandbox namedtuples are immutable.
#[pyfunction]
#[pyo3(name = "__delattr__")]
fn del_attr(slf: &Bound<'_, PyAny>, _name: &Bound<'_, PyAny>) -> PyResult<()> {
    Err(immutable_error(slf))
}

/// `Point(x=1, y=2)`, under the type's name without its module.
#[pyfunction]
#[pyo3(name = "__repr__")]
fn repr(slf: &Bound<'_, PyAny>) -> PyResult<String> {
    let mut parts = Vec::new();
    for (name, value) in fields(slf)?.into_iter().zip(slf.try_iter()?) {
        parts.push(format!("{name}={}", value?.repr()?));
    }
    Ok(format!("{}({})", simple_name(slf)?, parts.join(", ")))
}

/// Pickles as `MontyNamedTuple(type_name, fields, values)`.
#[pyfunction]
#[pyo3(name = "__reduce__")]
fn reduce<'py>(slf: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyTuple>> {
    let py = slf.py();
    let args = (type_name(slf)?, fields_object(slf)?, as_tuple(slf)?);
    PyTuple::new(
        py,
        [
            monty_named_tuple_type(py)?.clone().into_any(),
            args.into_pyobject(py)?.into_any(),
        ],
    )
}

/// The values, as on `collections.namedtuple`; pickling itself goes through `__reduce__`.
#[pyfunction]
#[pyo3(name = "__getnewargs__")]
fn get_new_args<'py>(slf: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyTuple>> {
    as_tuple(slf)
}

/// The tuple's attributes plus the field names, sorted.
#[pyfunction]
#[pyo3(name = "__dir__")]
fn dir<'py>(slf: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyList>> {
    let py = slf.py();
    let names = PySet::empty(py)?;
    for name in py
        .get_type::<PyTuple>()
        .call_method1(intern!(py, "__dir__"), (slf,))?
        .try_iter()?
    {
        names.add(name?)?;
    }
    for field in fields(slf)? {
        names.add(field)?;
    }
    let sorted = import_builtins(py)?
        .bind(py)
        .call_method1(intern!(py, "sorted"), (names,))?;
    Ok(sorted.cast_into::<PyList>()?)
}

/// A new namedtuple of the same type from a sequence or iterable.
#[pyfunction]
#[pyo3(name = "_make", text_signature = "(self, iterable)")]
fn make<'py>(slf: &Bound<'py, PyAny>, iterable: Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let py = slf.py();
    monty_named_tuple_type(py)?.call1((type_name(slf)?, fields_object(slf)?, iterable))
}

/// A new namedtuple of the same type with the given fields replaced.
#[pyfunction]
#[pyo3(name = "_replace", signature = (slf, /, **kwds), text_signature = "(self, /, **kwds)")]
fn replace<'py>(slf: &Bound<'py, PyAny>, kwds: Option<Bound<'py, PyDict>>) -> PyResult<Bound<'py, PyAny>> {
    let py = slf.py();
    let kwds = kwds.unwrap_or_else(|| PyDict::new(py));
    let mut values = Vec::new();
    for (field, value) in fields(slf)?.into_iter().zip(slf.try_iter()?) {
        let value = value?;
        values.push(match kwds.get_item(&field)? {
            Some(replacement) => {
                kwds.del_item(&field)?;
                replacement
            }
            None => value,
        });
    }
    let result = make(slf, PyTuple::new(py, values)?.into_any())?;
    if kwds.is_empty() {
        Ok(result)
    } else {
        let message = format!("Got unexpected field names: {}", kwds.keys().repr()?);
        // `_replace` with an unknown field raised `ValueError` until 3.13
        Err(if py.version_info() >= (3, 13) {
            PyTypeError::new_err(message)
        } else {
            PyValueError::new_err(message)
        })
    }
}

/// A new dict mapping field names to values.
#[pyfunction]
#[pyo3(name = "_asdict", text_signature = "(self)")]
fn as_dict<'py>(slf: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyDict>> {
    let dict = PyDict::new(slf.py());
    for (field, value) in fields(slf)?.into_iter().zip(slf.try_iter()?) {
        dict.set_item(field, value?)?;
    }
    Ok(dict)
}

/// Always empty: sandbox namedtuples cross without their defaults.
#[pyfunction]
#[pyo3(name = "_field_defaults")]
fn field_defaults<'py>(slf: &Bound<'py, PyAny>) -> Bound<'py, PyDict> {
    PyDict::new(slf.py())
}

/// The `count` field's value if there is one, else the bound `tuple.count`.
#[pyfunction]
fn count<'py>(slf: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    field_or_tuple_method(slf, intern!(slf.py(), "count"))
}

/// The `index` field's value if there is one, else the bound `tuple.index`.
#[pyfunction]
fn index<'py>(slf: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    field_or_tuple_method(slf, intern!(slf.py(), "index"))
}

/// The field `name`'s value if there is one, else the tuple method `name` bound to `slf`.
fn field_or_tuple_method<'py>(slf: &Bound<'py, PyAny>, name: &Bound<'py, PyString>) -> PyResult<Bound<'py, PyAny>> {
    match field_position(slf, name)? {
        Some(position) => slf.get_item(position),
        None => slf
            .py()
            .get_type::<PyTuple>()
            .getattr(name)?
            .call_method1(intern!(slf.py(), "__get__"), (slf,)),
    }
}

/// The position of the field `name`, if the instance has one.
fn field_position(slf: &Bound<'_, PyAny>, name: &Bound<'_, PyString>) -> PyResult<Option<usize>> {
    for (position, field) in fields(slf)?.into_iter().enumerate() {
        if field.eq(name)? {
            return Ok(Some(position));
        }
    }
    Ok(None)
}

/// The instance's `_fields` object, as stored.
fn fields_object<'py>(slf: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    slf.getattr(intern!(slf.py(), "_fields"))
}

/// The instance's field names, in order.
fn fields<'py>(slf: &Bound<'py, PyAny>) -> PyResult<Vec<Bound<'py, PyAny>>> {
    fields_object(slf)?.try_iter()?.collect()
}

/// The instance's `_type_name`.
fn type_name<'py>(slf: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    slf.getattr(intern!(slf.py(), "_type_name"))
}

/// The type name without its module, as `type(p).__name__` would give for a real namedtuple.
fn simple_name(slf: &Bound<'_, PyAny>) -> PyResult<String> {
    let name: String = type_name(slf)?.extract()?;
    Ok(name.rsplit('.').next().unwrap_or_default().to_owned())
}

/// The values as a plain tuple.
fn as_tuple<'py>(slf: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyTuple>> {
    PyTuple::new(slf.py(), slf.try_iter()?.collect::<PyResult<Vec<_>>>()?)
}

/// The simple name's `repr()`, as error messages quote it.
fn simple_name_repr(slf: &Bound<'_, PyAny>) -> PyResult<String> {
    Ok(PyString::new(slf.py(), &simple_name(slf)?).repr()?.to_string())
}

/// The `AttributeError` every attribute assignment or deletion raises.
fn immutable_error(slf: &Bound<'_, PyAny>) -> PyErr {
    match simple_name_repr(slf) {
        Ok(name) => PyAttributeError::new_err(format!("{name} object is immutable")),
        Err(err) => err,
    }
}
