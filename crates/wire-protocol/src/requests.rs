//! The `ParentRequest` oneof arms as Python classes (parent → child).
//!
//! Each arm exposes `to_kind` (build the `pb::parent_request::Kind` for
//! encoding) and, where it carries a payload, `from_*` (rebuild the arm from a
//! decoded payload). [`crate::wire::encode_parent_request`] downcasts a Python
//! object to the matching arm; [`crate::wire::decode_parent_request`]
//! dispatches on the decoded `kind`.

use monty::MontyObject;
use monty_proto::pb;
use pyo3::{
    prelude::*,
    types::{PyDict, PyMapping, PyString, PyType},
};

use crate::{
    get_version,
    messages::{
        ExtFunctionResult, FutureResult, Mount, from_monty, mount_mode_from_proto, mount_mode_to_proto, proto_err,
        to_monty, wire_object_into,
    },
};

/// Opens the session the child serves until `Reset`. Carries the parent's
/// `monty_version`; the child rejects a mismatch with a `FatalError`.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct StartSession {
    #[pyo3(get)]
    script_name: String,
    limits: Option<pb::ResourceLimits>,
    #[pyo3(get)]
    type_check: bool,
    #[pyo3(get)]
    type_check_stubs: Option<String>,
    #[pyo3(get)]
    monty_version: String,
}

#[pymethods]
impl StartSession {
    #[new]
    #[pyo3(signature = (
        *, script_name = "main.py".to_owned(), limits = None, type_check = false,
        type_check_stubs = None, monty_version = None,
    ))]
    fn new(
        script_name: String,
        limits: Option<&Bound<'_, PyMapping>>,
        type_check: bool,
        type_check_stubs: Option<String>,
        monty_version: Option<String>,
    ) -> PyResult<Self> {
        Ok(Self {
            script_name,
            limits: limits.map(limits_to_proto).transpose()?,
            type_check,
            type_check_stubs,
            // default to this build's version so the common case "just works".
            monty_version: monty_version.unwrap_or_else(|| get_version().to_owned()),
        })
    }

    /// The resource limits as a dict, or `None`.
    #[getter]
    fn limits(&self, py: Python<'_>) -> PyResult<Option<Py<PyDict>>> {
        self.limits.as_ref().map(|l| limits_from_proto(py, l)).transpose()
    }

    fn __repr__(&self) -> String {
        format!(
            "StartSession(script_name={:?}, type_check={}, monty_version={:?})",
            self.script_name, self.type_check, self.monty_version
        )
    }
}

impl StartSession {
    pub(crate) fn to_kind(&self) -> pb::parent_request::Kind {
        pb::parent_request::Kind::StartSession(pb::StartSession {
            script_name: self.script_name.clone(),
            limits: self.limits,
            type_check: self.type_check,
            type_check_stubs: self.type_check_stubs.clone(),
            monty_version: self.monty_version.clone(),
        })
    }

    pub(crate) fn from_proto(start: pb::StartSession) -> Self {
        Self {
            script_name: start.script_name,
            limits: start.limits,
            type_check: start.type_check,
            type_check_stubs: start.type_check_stubs,
            monty_version: start.monty_version,
        }
    }
}

/// Executes one snippet against the session.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct Feed {
    #[pyo3(get)]
    code: String,
    /// Input globals, as ordered `(name, value)` pairs.
    inputs: Vec<(String, MontyObject)>,
    mounts: Vec<Mount>,
    #[pyo3(get)]
    skip_type_check: bool,
}

#[pymethods]
impl Feed {
    #[new]
    #[pyo3(signature = (code, *, inputs = None, mounts = None, skip_type_check = false))]
    fn new(
        code: String,
        inputs: Option<&Bound<'_, PyMapping>>,
        mounts: Option<Vec<Mount>>,
        skip_type_check: bool,
    ) -> PyResult<Self> {
        Ok(Self {
            code,
            inputs: inputs.map(inputs_to_monty).transpose()?.unwrap_or_default(),
            mounts: mounts.unwrap_or_default(),
            skip_type_check,
        })
    }

    /// The input globals as a `dict[str, Any]`.
    #[getter]
    fn inputs(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let dict = PyDict::new(py);
        for (name, value) in &self.inputs {
            dict.set_item(name, from_monty(py, value)?)?;
        }
        Ok(dict.unbind())
    }

    #[getter]
    fn mounts(&self) -> Vec<Mount> {
        self.mounts.clone()
    }

    fn __repr__(&self) -> String {
        format!(
            "Feed(code={:?}, inputs=<{} item(s)>, mounts=<{} mount(s)>, skip_type_check={})",
            self.code,
            self.inputs.len(),
            self.mounts.len(),
            self.skip_type_check
        )
    }
}

impl Feed {
    pub(crate) fn to_kind(&self) -> PyResult<pb::parent_request::Kind> {
        let inputs = self
            .inputs
            .iter()
            .map(|(name, value)| pb::NamedValue {
                name: name.clone(),
                value: Some(value.clone().into()),
            })
            .collect();
        let mounts = self
            .mounts
            .iter()
            .map(|m| {
                Ok(pb::Mount {
                    virtual_path: m.virtual_path.clone(),
                    host_path: m.host_path.clone(),
                    mode: mount_mode_to_proto(&m.mode)?,
                    write_bytes_limit: m.write_bytes_limit,
                })
            })
            .collect::<PyResult<_>>()?;
        Ok(pb::parent_request::Kind::Feed(pb::Feed {
            code: self.code.clone(),
            inputs,
            mounts,
            skip_type_check: self.skip_type_check,
        }))
    }

    pub(crate) fn from_proto(feed: pb::Feed) -> PyResult<Self> {
        let inputs = feed
            .inputs
            .into_iter()
            .map(|nv| {
                let value = nv.value.ok_or_else(|| proto_err("Feed input has no value"))?;
                Ok((nv.name, wire_object_into(value)?))
            })
            .collect::<PyResult<_>>()?;
        let mounts = feed
            .mounts
            .into_iter()
            .map(|m| {
                Ok(Mount {
                    virtual_path: m.virtual_path,
                    host_path: m.host_path,
                    mode: mount_mode_from_proto(m.mode)?.to_owned(),
                    write_bytes_limit: m.write_bytes_limit,
                })
            })
            .collect::<PyResult<_>>()?;
        Ok(Self {
            code: feed.code,
            inputs,
            mounts,
            skip_type_check: feed.skip_type_check,
        })
    }
}

/// Answers a `FunctionCall` or `OsCall` suspension.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct ResumeCall {
    #[pyo3(get)]
    call_id: u32,
    #[pyo3(get)]
    result: ExtFunctionResult,
}

#[pymethods]
impl ResumeCall {
    #[new]
    fn new(call_id: u32, result: ExtFunctionResult) -> Self {
        Self { call_id, result }
    }

    fn __repr__(&self) -> String {
        format!("ResumeCall(call_id={})", self.call_id)
    }
}

impl ResumeCall {
    pub(crate) fn to_kind(&self) -> pb::parent_request::Kind {
        pb::parent_request::Kind::ResumeCall(pb::ResumeCall {
            call_id: self.call_id,
            result: Some(self.result.to_proto()),
        })
    }

    pub(crate) fn from_proto(resume: pb::ResumeCall) -> PyResult<Self> {
        let result = resume.result.ok_or_else(|| proto_err("ResumeCall has no result"))?;
        Ok(Self {
            call_id: resume.call_id,
            result: ExtFunctionResult::from_proto(result)?,
        })
    }
}

/// Answers a `NameLookup` suspension: either the resolved value, or
/// "undefined" (the child then raises `NameError`).
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct ResumeNameLookup {
    /// `Some(value)` resolves the name; `None` means undefined.
    value: Option<MontyObject>,
}

#[pymethods]
impl ResumeNameLookup {
    /// The name resolves to `value`.
    #[classmethod]
    fn resolved(_cls: &Bound<'_, PyType>, value: &Bound<'_, PyAny>) -> PyResult<Self> {
        Ok(Self {
            value: Some(to_monty(value)?),
        })
    }

    /// The name is undefined.
    #[classmethod]
    fn undefined(_cls: &Bound<'_, PyType>) -> Self {
        Self { value: None }
    }

    /// `True` when the name was resolved to a value.
    #[getter]
    fn is_defined(&self) -> bool {
        self.value.is_some()
    }

    /// The resolved value, or `None` when undefined. Note `None` is ambiguous
    /// with a resolved Python `None`; use `is_defined` to disambiguate.
    #[getter]
    fn value(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        self.value.as_ref().map(|v| from_monty(py, v)).transpose()
    }

    fn __repr__(&self) -> String {
        if self.value.is_some() {
            "ResumeNameLookup.resolved(...)".to_owned()
        } else {
            "ResumeNameLookup.undefined()".to_owned()
        }
    }
}

impl ResumeNameLookup {
    pub(crate) fn to_kind(&self) -> pb::parent_request::Kind {
        let kind = match &self.value {
            Some(value) => pb::resume_name_lookup::Kind::Value(value.clone().into()),
            None => pb::resume_name_lookup::Kind::Undefined(pb::Unit {}),
        };
        pb::parent_request::Kind::ResumeNameLookup(pb::ResumeNameLookup { kind: Some(kind) })
    }

    pub(crate) fn from_proto(lookup: pb::ResumeNameLookup) -> PyResult<Self> {
        let kind = lookup.kind.ok_or_else(|| proto_err("ResumeNameLookup has no kind"))?;
        let value = match kind {
            pb::resume_name_lookup::Kind::Value(value) => Some(wire_object_into(value)?),
            pb::resume_name_lookup::Kind::Undefined(_) => None,
        };
        Ok(Self { value })
    }
}

/// Answers a `ResolveFutures` suspension with results for pending call ids.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct ResumeFutures {
    #[pyo3(get)]
    results: Vec<FutureResult>,
}

#[pymethods]
impl ResumeFutures {
    #[new]
    fn new(results: Vec<FutureResult>) -> Self {
        Self { results }
    }

    fn __repr__(&self) -> String {
        format!("ResumeFutures(<{} result(s)>)", self.results.len())
    }
}

impl ResumeFutures {
    pub(crate) fn to_kind(&self) -> pb::parent_request::Kind {
        let results = self
            .results
            .iter()
            .map(|fr| pb::FutureResult {
                call_id: fr.call_id,
                result: Some(fr.result.to_proto()),
            })
            .collect();
        pb::parent_request::Kind::ResumeFutures(pb::ResumeFutures { results })
    }

    pub(crate) fn from_proto(resume: pb::ResumeFutures) -> PyResult<Self> {
        let results = resume
            .results
            .into_iter()
            .map(|fr| {
                let result = fr.result.ok_or_else(|| proto_err("FutureResult has no result"))?;
                Ok(FutureResult {
                    call_id: fr.call_id,
                    result: ExtFunctionResult::from_proto(result)?,
                })
            })
            .collect::<PyResult<_>>()?;
        Ok(Self { results })
    }
}

/// Requests an opaque snapshot of the current session state.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct Dump;

#[pymethods]
impl Dump {
    #[new]
    fn new() -> Self {
        Self
    }

    fn __repr__(&self) -> &'static str {
        let _ = self;
        "Dump()"
    }
}

impl Dump {
    pub(crate) fn to_kind(&self) -> pb::parent_request::Kind {
        let _ = self;
        pb::parent_request::Kind::Dump(pb::Dump {})
    }
}

/// Restores state produced by `Dump` into a fresh (no-session) child.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct Load {
    #[pyo3(get)]
    state: Vec<u8>,
}

#[pymethods]
impl Load {
    #[new]
    fn new(state: Vec<u8>) -> Self {
        Self { state }
    }

    fn __repr__(&self) -> String {
        format!("Load(<{} bytes>)", self.state.len())
    }
}

impl Load {
    pub(crate) fn to_kind(&self) -> pb::parent_request::Kind {
        pb::parent_request::Kind::Load(pb::Load {
            state: self.state.clone(),
        })
    }

    pub(crate) fn from_proto(load: pb::Load) -> Self {
        Self { state: load.state }
    }
}

/// Ends the checkout: the child drops session state and returns to no-session.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct Reset;

#[pymethods]
impl Reset {
    #[new]
    fn new() -> Self {
        Self
    }

    fn __repr__(&self) -> &'static str {
        let _ = self;
        "Reset()"
    }
}

impl Reset {
    pub(crate) fn to_kind(&self) -> pb::parent_request::Kind {
        let _ = self;
        pb::parent_request::Kind::Reset(pb::Reset {})
    }
}

/// Asks the child to reply `Ok` and exit cleanly.
#[pyclass(frozen, eq, from_py_object, module = "wire_protocol")]
#[derive(Clone, PartialEq)]
pub struct Shutdown;

#[pymethods]
impl Shutdown {
    #[new]
    fn new() -> Self {
        Self
    }

    fn __repr__(&self) -> &'static str {
        let _ = self;
        "Shutdown()"
    }
}

impl Shutdown {
    pub(crate) fn to_kind(&self) -> pb::parent_request::Kind {
        let _ = self;
        pb::parent_request::Kind::Shutdown(pb::Shutdown {})
    }
}

// =============================================================================
// limits dict ↔ pb::ResourceLimits
// =============================================================================

/// Parses a Python limits mapping into `pb::ResourceLimits`. Keys mirror the
/// proto fields (all optional ints, in the protocol's own units):
/// `max_allocations`, `max_duration_micros`, `max_memory_bytes`,
/// `gc_interval`, `max_recursion_depth`.
fn limits_to_proto(limits: &Bound<'_, PyMapping>) -> PyResult<pb::ResourceLimits> {
    Ok(pb::ResourceLimits {
        max_allocations: limit_field(limits, "max_allocations")?,
        max_duration_micros: limit_field(limits, "max_duration_micros")?,
        max_memory_bytes: limit_field(limits, "max_memory_bytes")?,
        gc_interval: limit_field(limits, "gc_interval")?,
        max_recursion_depth: limit_field(limits, "max_recursion_depth")?,
    })
}

/// Reads one optional `u64` limit field; absent or `None` means "unset".
fn limit_field(limits: &Bound<'_, PyMapping>, key: &str) -> PyResult<Option<u64>> {
    let key = PyString::new(limits.py(), key);
    match limits.get_item(&key) {
        Ok(value) if !value.is_none() => Ok(Some(value.extract()?)),
        _ => Ok(None),
    }
}

/// Renders `pb::ResourceLimits` back into a Python dict, omitting unset fields.
fn limits_from_proto(py: Python<'_>, limits: &pb::ResourceLimits) -> PyResult<Py<PyDict>> {
    let dict = PyDict::new(py);
    let entries = [
        ("max_allocations", limits.max_allocations),
        ("max_duration_micros", limits.max_duration_micros),
        ("max_memory_bytes", limits.max_memory_bytes),
        ("gc_interval", limits.gc_interval),
        ("max_recursion_depth", limits.max_recursion_depth),
    ];
    for (key, value) in entries {
        if let Some(value) = value {
            dict.set_item(key, value)?;
        }
    }
    Ok(dict.unbind())
}

/// Builds the matching Python request object from a decoded
/// `pb::parent_request::Kind`.
pub(crate) fn request_from_proto(py: Python<'_>, kind: pb::parent_request::Kind) -> PyResult<Py<PyAny>> {
    let obj = match kind {
        pb::parent_request::Kind::StartSession(s) => Py::new(py, StartSession::from_proto(s))?.into_any(),
        pb::parent_request::Kind::Feed(f) => Py::new(py, Feed::from_proto(f)?)?.into_any(),
        pb::parent_request::Kind::ResumeCall(r) => Py::new(py, ResumeCall::from_proto(r)?)?.into_any(),
        pb::parent_request::Kind::ResumeNameLookup(r) => Py::new(py, ResumeNameLookup::from_proto(r)?)?.into_any(),
        pb::parent_request::Kind::ResumeFutures(r) => Py::new(py, ResumeFutures::from_proto(r)?)?.into_any(),
        pb::parent_request::Kind::Dump(_) => Py::new(py, Dump)?.into_any(),
        pb::parent_request::Kind::Load(l) => Py::new(py, Load::from_proto(l))?.into_any(),
        pb::parent_request::Kind::Reset(_) => Py::new(py, Reset)?.into_any(),
        pb::parent_request::Kind::Shutdown(_) => Py::new(py, Shutdown)?.into_any(),
    };
    Ok(obj)
}

/// Parses a Python mapping of `str -> value` input globals into ordered
/// `(name, MontyObject)` pairs.
fn inputs_to_monty(inputs: &Bound<'_, PyMapping>) -> PyResult<Vec<(String, MontyObject)>> {
    inputs
        .items()?
        .try_iter()?
        .map(|item| {
            let (name, value): (String, Bound<'_, PyAny>) = item?.extract()?;
            Ok((name, to_monty(&value)?))
        })
        .collect()
}
