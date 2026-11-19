//! [`PyExternalModule`]: a host module as `external_modules` takes it, and
//! as a module value crossing back out of the sandbox decodes to.

use monty_types::{MontyUuid, is_identifier, validate_module_name};
use pyo3::{
    exceptions::{PyTypeError, PyValueError},
    prelude::*,
    sync::PyOnceLock,
    types::{PyDict, PyString},
};

use super::class_instance::{py_uuid, uuid_to_py};

/// A host module: its attributes, its optional `.pyi` stubs and its identity.
/// `attrs` is copied and validated once, at construction, so a module is
/// immutable; the `external_modules` mapping is the only live surface.
#[pyclass(name = "ExternalModule", module = "pydantic_monty", frozen)]
pub struct PyExternalModule {
    /// The module's attributes, a private copy of the dict given.
    attrs: Py<PyDict>,
    /// The module's `.pyi` source, if the host supplies one.
    stubs: Option<String>,
    /// Identity of the module, carried by every import of it.
    id: MontyUuid,
}

#[pymethods]
impl PyExternalModule {
    #[new]
    #[pyo3(signature = (attrs, *, stubs = None, id = None))]
    fn py_new(attrs: &Bound<'_, PyAny>, stubs: Option<String>, id: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let py = attrs.py();
        let id = match id {
            Some(id) => py_uuid(id, "ExternalModule id")?,
            None => uuid4(py)?,
        };
        Self::new(py, attrs, stubs, id)
    }

    /// A copy of the module's attributes.
    #[getter]
    fn attrs(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        self.attrs.bind(py).copy().map(Bound::unbind)
    }

    /// The module's `.pyi` source, `None` when it has none.
    #[getter]
    fn stubs(&self) -> Option<&str> {
        self.stubs.as_deref()
    }

    /// Identity of the module as a `uuid.UUID`.
    #[getter]
    fn id(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        uuid_to_py(py, &self.id)
    }

    /// `ExternalModule({'add': <function add>}, stubs=None, id=UUID('...'))`
    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        let attrs: String = self.attrs.bind(py).repr()?.extract()?;
        let stubs = match &self.stubs {
            Some(stubs) => PyString::new(py, stubs).repr()?.extract()?,
            None => "None".to_owned(),
        };
        Ok(format!(
            "ExternalModule({attrs}, stubs={stubs}, id=UUID('{}'))",
            self.id
        ))
    }

    /// Equal when id, attrs and stubs all match.
    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        if let Ok(other) = other.extract::<PyRef<'_, Self>>() {
            Ok(self.id == other.id && self.stubs == other.stubs && self.attrs.bind(py).eq(other.attrs.bind(py))?)
        } else {
            Ok(false)
        }
    }
}

impl PyExternalModule {
    /// Builds a module from an attrs dict, checking it as the constructor does:
    /// `str` identifier keys, and a nested module under a key that is a valid
    /// module name (it is imported by it). The copy holds plain `str` keys, so
    /// a `str` subclass cannot bend the checks or later lookups.
    pub fn new(py: Python<'_>, attrs: &Bound<'_, PyAny>, stubs: Option<String>, id: MontyUuid) -> PyResult<Self> {
        let Ok(attrs) = attrs.cast::<PyDict>() else {
            return Err(PyTypeError::new_err(format!(
                "ExternalModule attrs must be a dict, not {}",
                attrs.get_type().name()?
            )));
        };
        let copy = PyDict::new(py);
        for (key, value) in attrs.iter() {
            let Ok(key) = key.cast::<PyString>() else {
                return Err(PyTypeError::new_err(format!(
                    "ExternalModule attrs must have str keys, not {}",
                    key.get_type().name()?
                )));
            };
            let name = key.to_cow()?;
            // two keys that differ only as `str` subclasses would collide as plain text
            if copy.contains(&*name)? {
                return Err(PyValueError::new_err(format!(
                    "ExternalModule attrs has the key {} twice",
                    key.repr()?
                )));
            }
            if !is_identifier(&name) {
                return Err(PyValueError::new_err(format!(
                    "ExternalModule attrs must have identifier keys, not {}",
                    key.repr()?
                )));
            }
            if value.is_instance_of::<Self>() {
                // a submodule is imported by its key, so that needs to be a module name too
                validate_module_name(&name)
                    .map_err(|err| PyValueError::new_err(format!("ExternalModule attrs['{name}']: {err}")))?;
            }
            copy.set_item(PyString::new(py, &name), value)?;
        }
        Ok(Self {
            attrs: copy.unbind(),
            stubs,
            id,
        })
    }

    /// Identity of the module.
    #[must_use]
    pub fn uuid(&self) -> MontyUuid {
        self.id
    }

    /// The module's `.pyi` source, if any.
    #[must_use]
    pub fn stubs_source(&self) -> Option<&str> {
        self.stubs.as_deref()
    }

    /// The public attribute `name`: `None` when absent or starting with `_`.
    pub fn attr<'py>(&self, py: Python<'py>, name: &str) -> PyResult<Option<Bound<'py, PyAny>>> {
        if name.starts_with('_') {
            Ok(None)
        } else {
            self.attrs.bind(py).get_item(name)
        }
    }

    /// The public attributes in insertion order; keys are identifiers.
    #[must_use]
    pub fn public_attrs<'py>(&self, py: Python<'py>) -> Vec<(String, Bound<'py, PyAny>)> {
        self.attrs
            .bind(py)
            .iter()
            .map(|(key, value)| (key.to_string(), value))
            .filter(|(key, _)| !key.starts_with('_'))
            .collect()
    }
}

/// A fresh random id from `uuid.uuid4()`.
fn uuid4(py: Python<'_>) -> PyResult<MontyUuid> {
    static UUID4: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
    py_uuid(&UUID4.import(py, "uuid", "uuid4")?.call0()?, "uuid4()")
}
