//! Unified snapshot serialization with versioning and integrity checks.
//!
//! All snapshot `dump()` calls produce a wire format:
//!
//! ```text
//! [version: u16 LE] [sha256: 32 bytes] [postcard payload]
//! ```
//!
//! Two module-level `#[pyfunction]`s — `load_snapshot` and `load_repl_snapshot` —
//! handle deserialization without requiring callers to know the snapshot type.

use std::sync::{Mutex, PoisonError};

use ::monty::{
    FunctionCall, LimitedTracker, MontyObject, NameLookup, NoLimitTracker, OsCall, ReplFunctionCall, ReplNameLookup,
    ReplOsCall, ReplResolveFutures, ResolveFutures,
};
use pyo3::{
    exceptions::{PyRuntimeError, PyValueError},
    prelude::*,
    types::{PyBytes, PyDict, PyList, PyTuple},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    convert::{monty_to_py, py_to_monty},
    dataclass::DcRegistry,
    limits::PySignalTracker,
    monty_cls::{
        EitherFunctionSnapshot, EitherFutureSnapshot, EitherLookupSnapshot, PyFunctionSnapshot, PyFutureSnapshot,
        PyNameLookupSnapshot,
    },
    repl::{EitherRepl, PyMontyRepl},
};

/// Current serialization format version. Incremented on breaking wire-format changes.
const SERIALIZATION_VERSION: u16 = 1;

/// Size of the wire-format header: 2 bytes version + 32 bytes SHA-256 hash.
const HEADER_SIZE: usize = 2 + 32;

// ---------------------------------------------------------------------------
// Wire-format helpers
// ---------------------------------------------------------------------------

/// Serializes a value with a version header and SHA-256 integrity hash.
///
/// Layout: `[version: u16 LE] [sha256(payload): 32 bytes] [postcard payload]`
fn serialize_with_header(value: &impl Serialize) -> Result<Vec<u8>, postcard::Error> {
    let payload = postcard::to_allocvec(value)?;

    let hash = Sha256::digest(&payload);

    let mut buf = Vec::with_capacity(HEADER_SIZE + payload.len());
    buf.extend_from_slice(&SERIALIZATION_VERSION.to_le_bytes());
    buf.extend_from_slice(&hash);
    buf.extend_from_slice(&payload);
    Ok(buf)
}

/// Deserializes bytes produced by `serialize_with_header`, checking version and integrity.
fn deserialize_with_header<'de, T: Deserialize<'de>>(bytes: &'de [u8]) -> PyResult<T> {
    if bytes.len() < HEADER_SIZE {
        return Err(PyValueError::new_err(
            "Serialized data is too short to contain a valid header",
        ));
    }

    let version = u16::from_le_bytes([bytes[0], bytes[1]]);
    if version != SERIALIZATION_VERSION {
        return Err(PyValueError::new_err(format!(
            "Serialized data version {version} is not compatible with current version {SERIALIZATION_VERSION}"
        )));
    }

    let stored_hash = &bytes[2..HEADER_SIZE];
    let payload = &bytes[HEADER_SIZE..];

    let computed_hash = Sha256::digest(payload);
    if computed_hash.as_slice() != stored_hash {
        return Err(PyValueError::new_err("Serialized data integrity check failed"));
    }

    postcard::from_bytes(payload).map_err(|e| PyValueError::new_err(e.to_string()))
}

// ---------------------------------------------------------------------------
// Tagged wrapper enums
// ---------------------------------------------------------------------------

/// Non-REPL snapshot: tagged union over all snapshot types.
///
/// Postcard's enum tagging handles type discrimination, so `load_snapshot`
/// doesn't need to know the snapshot type upfront.
#[derive(Serialize, Deserialize)]
pub(crate) enum SerializedSnapshot {
    /// External function or OS call.
    Function {
        snapshot: EitherFunctionSnapshot,
        script_name: String,
        is_os_function: bool,
        is_method_call: bool,
        function_name: String,
        args: Vec<MontyObject>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        call_id: u32,
    },
    /// Name lookup.
    NameLookup {
        snapshot: EitherLookupSnapshot,
        script_name: String,
        variable_name: String,
    },
    /// Future resolution.
    Future {
        snapshot: EitherFutureSnapshot,
        script_name: String,
    },
}

/// REPL snapshot: includes the REPL state alongside the execution snapshot.
///
/// On deserialization, the REPL state is reconstructed into a fresh `PyMontyRepl`
/// and the snapshot is rewired to reference it.
///
/// Uses `SerdeFunctionSnapshot` (etc.) directly so REPL call variants are preserved
/// in the wire format — unlike `EitherFunctionSnapshot::Deserialize` which maps
/// REPL variants to `Done`.
#[derive(Serialize, Deserialize)]
pub(crate) enum SerializedReplSnapshot {
    /// External function or OS call with REPL state.
    Function {
        snapshot: SerdeFunctionSnapshot,
        repl: EitherRepl,
        script_name: String,
        is_os_function: bool,
        is_method_call: bool,
        function_name: String,
        args: Vec<MontyObject>,
        kwargs: Vec<(MontyObject, MontyObject)>,
        call_id: u32,
    },
    /// Name lookup with REPL state.
    NameLookup {
        snapshot: SerdeLookupSnapshot,
        repl: EitherRepl,
        script_name: String,
        variable_name: String,
    },
    /// Future resolution with REPL state.
    Future {
        snapshot: SerdeFutureSnapshot,
        repl: EitherRepl,
        script_name: String,
    },
}

// ---------------------------------------------------------------------------
// Serde helpers for Either*Snapshot types
// ---------------------------------------------------------------------------

/// Serde helper: mirrors `EitherFunctionSnapshot` layout but without `Py<PyMontyRepl>`.
///
/// REPL variants are serialized with the inner call data preserved; for non-REPL loads
/// they produce `Done`, but for REPL loads they are rewired with a fresh `Py<PyMontyRepl>`.
#[derive(Serialize, Deserialize)]
pub(crate) enum SerdeFunctionSnapshot {
    NoLimitFn(FunctionCall<PySignalTracker<NoLimitTracker>>),
    NoLimitOs(OsCall<PySignalTracker<NoLimitTracker>>),
    LimitedFn(FunctionCall<PySignalTracker<LimitedTracker>>),
    LimitedOs(OsCall<PySignalTracker<LimitedTracker>>),
    ReplNoLimitFn(ReplFunctionCall<PySignalTracker<NoLimitTracker>>),
    ReplNoLimitOs(ReplOsCall<PySignalTracker<NoLimitTracker>>),
    ReplLimitedFn(ReplFunctionCall<PySignalTracker<LimitedTracker>>),
    ReplLimitedOs(ReplOsCall<PySignalTracker<LimitedTracker>>),
    Done,
}

/// Serde helper: borrows from `EitherFunctionSnapshot` for zero-copy serialization.
#[derive(Serialize)]
enum SerdeFunctionSnapshotRef<'a> {
    NoLimitFn(&'a FunctionCall<PySignalTracker<NoLimitTracker>>),
    NoLimitOs(&'a OsCall<PySignalTracker<NoLimitTracker>>),
    LimitedFn(&'a FunctionCall<PySignalTracker<LimitedTracker>>),
    LimitedOs(&'a OsCall<PySignalTracker<LimitedTracker>>),
    ReplNoLimitFn(&'a ReplFunctionCall<PySignalTracker<NoLimitTracker>>),
    ReplNoLimitOs(&'a ReplOsCall<PySignalTracker<NoLimitTracker>>),
    ReplLimitedFn(&'a ReplFunctionCall<PySignalTracker<LimitedTracker>>),
    ReplLimitedOs(&'a ReplOsCall<PySignalTracker<LimitedTracker>>),
    Done,
}

impl Serialize for EitherFunctionSnapshot {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let r = match self {
            Self::NoLimitFn(c) => SerdeFunctionSnapshotRef::NoLimitFn(c),
            Self::NoLimitOs(c) => SerdeFunctionSnapshotRef::NoLimitOs(c),
            Self::LimitedFn(c) => SerdeFunctionSnapshotRef::LimitedFn(c),
            Self::LimitedOs(c) => SerdeFunctionSnapshotRef::LimitedOs(c),
            Self::ReplNoLimitFn(c, _) => SerdeFunctionSnapshotRef::ReplNoLimitFn(c),
            Self::ReplNoLimitOs(c, _) => SerdeFunctionSnapshotRef::ReplNoLimitOs(c),
            Self::ReplLimitedFn(c, _) => SerdeFunctionSnapshotRef::ReplLimitedFn(c),
            Self::ReplLimitedOs(c, _) => SerdeFunctionSnapshotRef::ReplLimitedOs(c),
            Self::Done => SerdeFunctionSnapshotRef::Done,
        };
        r.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for EitherFunctionSnapshot {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let v = SerdeFunctionSnapshot::deserialize(deserializer)?;
        Ok(match v {
            SerdeFunctionSnapshot::NoLimitFn(c) => Self::NoLimitFn(c),
            SerdeFunctionSnapshot::NoLimitOs(c) => Self::NoLimitOs(c),
            SerdeFunctionSnapshot::LimitedFn(c) => Self::LimitedFn(c),
            SerdeFunctionSnapshot::LimitedOs(c) => Self::LimitedOs(c),
            // REPL variants deserialize as Done — the REPL owner is restored separately
            // by `load_repl_snapshot` which uses `SerdeFunctionSnapshot` directly.
            SerdeFunctionSnapshot::ReplNoLimitFn(_)
            | SerdeFunctionSnapshot::ReplNoLimitOs(_)
            | SerdeFunctionSnapshot::ReplLimitedFn(_)
            | SerdeFunctionSnapshot::ReplLimitedOs(_)
            | SerdeFunctionSnapshot::Done => Self::Done,
        })
    }
}

impl SerdeFunctionSnapshot {
    /// Converts a deserialized serde snapshot into an `EitherFunctionSnapshot` with a REPL owner.
    ///
    /// REPL variants are attached to the given `Py<PyMontyRepl>`.
    /// Non-REPL variants pass through unchanged.
    fn into_either_with_repl(self, owner: Py<PyMontyRepl>) -> EitherFunctionSnapshot {
        match self {
            Self::NoLimitFn(c) => EitherFunctionSnapshot::NoLimitFn(c),
            Self::NoLimitOs(c) => EitherFunctionSnapshot::NoLimitOs(c),
            Self::LimitedFn(c) => EitherFunctionSnapshot::LimitedFn(c),
            Self::LimitedOs(c) => EitherFunctionSnapshot::LimitedOs(c),
            Self::ReplNoLimitFn(c) => EitherFunctionSnapshot::ReplNoLimitFn(c, owner),
            Self::ReplNoLimitOs(c) => EitherFunctionSnapshot::ReplNoLimitOs(c, owner),
            Self::ReplLimitedFn(c) => EitherFunctionSnapshot::ReplLimitedFn(c, owner),
            Self::ReplLimitedOs(c) => EitherFunctionSnapshot::ReplLimitedOs(c, owner),
            Self::Done => EitherFunctionSnapshot::Done,
        }
    }
}

/// Serde helper: serializable subset of `EitherLookupSnapshot` without REPL owner.
#[derive(Serialize, Deserialize)]
pub(crate) enum SerdeLookupSnapshot {
    NoLimit(NameLookup<PySignalTracker<NoLimitTracker>>),
    Limited(NameLookup<PySignalTracker<LimitedTracker>>),
    ReplNoLimit(ReplNameLookup<PySignalTracker<NoLimitTracker>>),
    ReplLimited(ReplNameLookup<PySignalTracker<LimitedTracker>>),
    Done,
}

/// Serde helper: borrows from `EitherLookupSnapshot` for zero-copy serialization.
#[derive(Serialize)]
enum SerdeLookupSnapshotRef<'a> {
    NoLimit(&'a NameLookup<PySignalTracker<NoLimitTracker>>),
    Limited(&'a NameLookup<PySignalTracker<LimitedTracker>>),
    ReplNoLimit(&'a ReplNameLookup<PySignalTracker<NoLimitTracker>>),
    ReplLimited(&'a ReplNameLookup<PySignalTracker<LimitedTracker>>),
    Done,
}

impl Serialize for EitherLookupSnapshot {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let r = match self {
            Self::NoLimit(l) => SerdeLookupSnapshotRef::NoLimit(l),
            Self::Limited(l) => SerdeLookupSnapshotRef::Limited(l),
            Self::ReplNoLimit(l, _) => SerdeLookupSnapshotRef::ReplNoLimit(l),
            Self::ReplLimited(l, _) => SerdeLookupSnapshotRef::ReplLimited(l),
            Self::Done => SerdeLookupSnapshotRef::Done,
        };
        r.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for EitherLookupSnapshot {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let v = SerdeLookupSnapshot::deserialize(deserializer)?;
        Ok(match v {
            SerdeLookupSnapshot::NoLimit(l) => Self::NoLimit(l),
            SerdeLookupSnapshot::Limited(l) => Self::Limited(l),
            SerdeLookupSnapshot::ReplNoLimit(_) | SerdeLookupSnapshot::ReplLimited(_) | SerdeLookupSnapshot::Done => {
                Self::Done
            }
        })
    }
}

impl SerdeLookupSnapshot {
    /// Converts a deserialized serde snapshot into an `EitherLookupSnapshot` with a REPL owner.
    fn into_either_with_repl(self, owner: Py<PyMontyRepl>) -> EitherLookupSnapshot {
        match self {
            Self::NoLimit(l) => EitherLookupSnapshot::NoLimit(l),
            Self::Limited(l) => EitherLookupSnapshot::Limited(l),
            Self::ReplNoLimit(l) => EitherLookupSnapshot::ReplNoLimit(l, owner),
            Self::ReplLimited(l) => EitherLookupSnapshot::ReplLimited(l, owner),
            Self::Done => EitherLookupSnapshot::Done,
        }
    }
}

/// Serde helper for `EitherFutureSnapshot` without REPL owner.
#[derive(Serialize, Deserialize)]
pub(crate) enum SerdeFutureSnapshot {
    NoLimit(ResolveFutures<PySignalTracker<NoLimitTracker>>),
    Limited(ResolveFutures<PySignalTracker<LimitedTracker>>),
    ReplNoLimit(ReplResolveFutures<PySignalTracker<NoLimitTracker>>),
    ReplLimited(ReplResolveFutures<PySignalTracker<LimitedTracker>>),
    Done,
}

/// Serde helper: borrows from `EitherFutureSnapshot` for zero-copy serialization.
#[derive(Serialize)]
enum SerdeFutureSnapshotRef<'a> {
    NoLimit(&'a ResolveFutures<PySignalTracker<NoLimitTracker>>),
    Limited(&'a ResolveFutures<PySignalTracker<LimitedTracker>>),
    ReplNoLimit(&'a ReplResolveFutures<PySignalTracker<NoLimitTracker>>),
    ReplLimited(&'a ReplResolveFutures<PySignalTracker<LimitedTracker>>),
    Done,
}

impl Serialize for EitherFutureSnapshot {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let r = match self {
            Self::NoLimit(s) => SerdeFutureSnapshotRef::NoLimit(s),
            Self::Limited(s) => SerdeFutureSnapshotRef::Limited(s),
            Self::ReplNoLimit(s, _) => SerdeFutureSnapshotRef::ReplNoLimit(s),
            Self::ReplLimited(s, _) => SerdeFutureSnapshotRef::ReplLimited(s),
            Self::Done => SerdeFutureSnapshotRef::Done,
        };
        r.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for EitherFutureSnapshot {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let v = SerdeFutureSnapshot::deserialize(deserializer)?;
        Ok(match v {
            SerdeFutureSnapshot::NoLimit(s) => Self::NoLimit(s),
            SerdeFutureSnapshot::Limited(s) => Self::Limited(s),
            SerdeFutureSnapshot::ReplNoLimit(_) | SerdeFutureSnapshot::ReplLimited(_) | SerdeFutureSnapshot::Done => {
                Self::Done
            }
        })
    }
}

impl SerdeFutureSnapshot {
    /// Converts a deserialized serde snapshot into an `EitherFutureSnapshot` with a REPL owner.
    fn into_either_with_repl(self, owner: Py<PyMontyRepl>) -> EitherFutureSnapshot {
        match self {
            Self::NoLimit(s) => EitherFutureSnapshot::NoLimit(s),
            Self::Limited(s) => EitherFutureSnapshot::Limited(s),
            Self::ReplNoLimit(s) => EitherFutureSnapshot::ReplNoLimit(s, owner),
            Self::ReplLimited(s) => EitherFutureSnapshot::ReplLimited(s, owner),
            Self::Done => EitherFutureSnapshot::Done,
        }
    }
}

// ---------------------------------------------------------------------------
// dump helpers (called from #[pymethods] on each snapshot type)
// ---------------------------------------------------------------------------

/// Checks that a function snapshot hasn't been consumed, then serializes it.
///
/// For REPL variants, extracts the REPL state and produces `SerializedReplSnapshot`.
/// For non-REPL variants, produces `SerializedSnapshot`.
#[expect(clippy::too_many_arguments)]
pub(crate) fn dump_function_snapshot(
    py: Python<'_>,
    snapshot_mutex: &Mutex<EitherFunctionSnapshot>,
    script_name: &str,
    is_os_function: bool,
    is_method_call: bool,
    function_name: &str,
    args: &Py<PyTuple>,
    kwargs: &Py<PyDict>,
    call_id: u32,
    dc_registry: &DcRegistry,
) -> PyResult<Vec<u8>> {
    let snapshot = snapshot_mutex.lock().unwrap_or_else(PoisonError::into_inner);
    if matches!(&*snapshot, EitherFunctionSnapshot::Done) {
        return Err(PyRuntimeError::new_err(
            "Cannot dump progress that has already been resumed",
        ));
    }

    let args_monty = convert_args_to_monty(py, args, dc_registry)?;
    let kwargs_monty = convert_kwargs_to_monty(py, kwargs, dc_registry)?;

    if snapshot.is_repl() {
        let repl = snapshot.take_repl_state()?;
        let serialized = SerializedReplSnapshotRef::Function {
            snapshot: &snapshot,
            repl: &repl,
            script_name,
            is_os_function,
            is_method_call,
            function_name,
            args: &args_monty,
            kwargs: &kwargs_monty,
            call_id,
        };
        serialize_with_header(&serialized).map_err(|e| PyValueError::new_err(e.to_string()))
    } else {
        let serialized = SerializedSnapshotRef::Function {
            snapshot: &snapshot,
            script_name,
            is_os_function,
            is_method_call,
            function_name,
            args: &args_monty,
            kwargs: &kwargs_monty,
            call_id,
        };
        serialize_with_header(&serialized).map_err(|e| PyValueError::new_err(e.to_string()))
    }
}

/// Checks that a lookup snapshot hasn't been consumed, then serializes it.
pub(crate) fn dump_lookup_snapshot(
    snapshot_mutex: &Mutex<EitherLookupSnapshot>,
    script_name: &str,
    variable_name: &str,
) -> PyResult<Vec<u8>> {
    let snapshot = snapshot_mutex.lock().unwrap_or_else(PoisonError::into_inner);
    if matches!(&*snapshot, EitherLookupSnapshot::Done) {
        return Err(PyRuntimeError::new_err(
            "Cannot dump progress that has already been resumed",
        ));
    }

    if snapshot.is_repl() {
        let repl = snapshot.take_repl_state()?;
        let serialized = SerializedReplSnapshotRef::NameLookup {
            snapshot: &snapshot,
            repl: &repl,
            script_name,
            variable_name,
        };
        serialize_with_header(&serialized).map_err(|e| PyValueError::new_err(e.to_string()))
    } else {
        let serialized = SerializedSnapshotRef::NameLookup {
            snapshot: &snapshot,
            script_name,
            variable_name,
        };
        serialize_with_header(&serialized).map_err(|e| PyValueError::new_err(e.to_string()))
    }
}

/// Checks that a future snapshot hasn't been consumed, then serializes it.
pub(crate) fn dump_future_snapshot(
    snapshot_mutex: &Mutex<EitherFutureSnapshot>,
    script_name: &str,
) -> PyResult<Vec<u8>> {
    let snapshot = snapshot_mutex.lock().unwrap_or_else(PoisonError::into_inner);
    if matches!(&*snapshot, EitherFutureSnapshot::Done) {
        return Err(PyRuntimeError::new_err(
            "Cannot dump progress that has already been resumed",
        ));
    }

    if snapshot.is_repl() {
        let repl = snapshot.take_repl_state()?;
        let serialized = SerializedReplSnapshotRef::Future {
            snapshot: &snapshot,
            repl: &repl,
            script_name,
        };
        serialize_with_header(&serialized).map_err(|e| PyValueError::new_err(e.to_string()))
    } else {
        let serialized = SerializedSnapshotRef::Future {
            snapshot: &snapshot,
            script_name,
        };
        serialize_with_header(&serialized).map_err(|e| PyValueError::new_err(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Borrowing serialization refs (avoid cloning large snapshot data)
// ---------------------------------------------------------------------------

/// Borrowing version of `SerializedSnapshot` for zero-copy serialization.
#[derive(Serialize)]
enum SerializedSnapshotRef<'a> {
    Function {
        snapshot: &'a EitherFunctionSnapshot,
        script_name: &'a str,
        is_os_function: bool,
        is_method_call: bool,
        function_name: &'a str,
        args: &'a [MontyObject],
        kwargs: &'a [(MontyObject, MontyObject)],
        call_id: u32,
    },
    NameLookup {
        snapshot: &'a EitherLookupSnapshot,
        script_name: &'a str,
        variable_name: &'a str,
    },
    Future {
        snapshot: &'a EitherFutureSnapshot,
        script_name: &'a str,
    },
}

/// Borrowing version of `SerializedReplSnapshot` for zero-copy serialization.
#[derive(Serialize)]
enum SerializedReplSnapshotRef<'a> {
    Function {
        snapshot: &'a EitherFunctionSnapshot,
        repl: &'a EitherRepl,
        script_name: &'a str,
        is_os_function: bool,
        is_method_call: bool,
        function_name: &'a str,
        args: &'a [MontyObject],
        kwargs: &'a [(MontyObject, MontyObject)],
        call_id: u32,
    },
    NameLookup {
        snapshot: &'a EitherLookupSnapshot,
        repl: &'a EitherRepl,
        script_name: &'a str,
        variable_name: &'a str,
    },
    Future {
        snapshot: &'a EitherFutureSnapshot,
        repl: &'a EitherRepl,
        script_name: &'a str,
    },
}

// ---------------------------------------------------------------------------
// Module-level load functions
// ---------------------------------------------------------------------------

/// Loads a non-REPL snapshot from bytes.
///
/// Returns `FunctionSnapshot | NameLookupSnapshot | FutureSnapshot` depending
/// on what was serialized. Callers no longer need to know the snapshot type upfront.
#[pyfunction]
#[pyo3(signature = (data, *, print_callback=None, dataclass_registry=None))]
pub(crate) fn load_snapshot<'py>(
    py: Python<'py>,
    data: &Bound<'_, PyBytes>,
    print_callback: Option<Py<PyAny>>,
    dataclass_registry: Option<&Bound<'_, PyList>>,
) -> PyResult<Bound<'py, PyAny>> {
    let bytes = data.as_bytes();
    let serialized: SerializedSnapshot = deserialize_with_header(bytes)?;
    let dc_registry = DcRegistry::from_list(py, dataclass_registry)?;

    match serialized {
        SerializedSnapshot::Function {
            snapshot,
            script_name,
            is_os_function,
            is_method_call,
            function_name,
            args,
            kwargs,
            call_id,
        } => {
            let py_args = monty_objects_to_py_tuple(py, &args, &dc_registry)?;
            let py_kwargs = monty_pairs_to_py_dict(py, &kwargs, &dc_registry)?;
            PyFunctionSnapshot::from_deserialized(
                py,
                snapshot,
                print_callback,
                dc_registry,
                script_name,
                is_os_function,
                is_method_call,
                function_name,
                py_args,
                py_kwargs,
                call_id,
            )
        }
        SerializedSnapshot::NameLookup {
            snapshot,
            script_name,
            variable_name,
        } => PyNameLookupSnapshot::from_deserialized(
            py,
            snapshot,
            print_callback,
            dc_registry,
            script_name,
            variable_name,
        ),
        SerializedSnapshot::Future { snapshot, script_name } => {
            PyFutureSnapshot::from_deserialized(py, snapshot, print_callback, dc_registry, script_name)
        }
    }
}

/// Loads a REPL snapshot from bytes, returning `(snapshot, MontyRepl)`.
///
/// The REPL state is reconstructed into a fresh `PyMontyRepl` and the snapshot's
/// REPL variant is rewired to point to it.
#[pyfunction]
#[pyo3(signature = (data, *, print_callback=None, dataclass_registry=None))]
pub(crate) fn load_repl_snapshot<'py>(
    py: Python<'py>,
    data: &Bound<'_, PyBytes>,
    print_callback: Option<Py<PyAny>>,
    dataclass_registry: Option<&Bound<'_, PyList>>,
) -> PyResult<(Bound<'py, PyAny>, Py<PyMontyRepl>)> {
    let bytes = data.as_bytes();
    let serialized: SerializedReplSnapshot = deserialize_with_header(bytes)?;
    let dc_registry = DcRegistry::from_list(py, dataclass_registry)?;

    match serialized {
        SerializedReplSnapshot::Function {
            snapshot,
            repl,
            script_name,
            is_os_function,
            is_method_call,
            function_name,
            args,
            kwargs,
            call_id,
        } => {
            let repl_py = create_py_repl(py, repl, &script_name, &dc_registry)?;
            let either = snapshot.into_either_with_repl(repl_py.clone_ref(py));
            let py_args = monty_objects_to_py_tuple(py, &args, &dc_registry)?;
            let py_kwargs = monty_pairs_to_py_dict(py, &kwargs, &dc_registry)?;
            let snap = PyFunctionSnapshot::from_deserialized(
                py,
                either,
                print_callback,
                dc_registry,
                script_name,
                is_os_function,
                is_method_call,
                function_name,
                py_args,
                py_kwargs,
                call_id,
            )?;
            Ok((snap, repl_py))
        }
        SerializedReplSnapshot::NameLookup {
            snapshot,
            repl,
            script_name,
            variable_name,
        } => {
            let repl_py = create_py_repl(py, repl, &script_name, &dc_registry)?;
            let either = snapshot.into_either_with_repl(repl_py.clone_ref(py));
            let snap = PyNameLookupSnapshot::from_deserialized(
                py,
                either,
                print_callback,
                dc_registry,
                script_name,
                variable_name,
            )?;
            Ok((snap, repl_py))
        }
        SerializedReplSnapshot::Future {
            snapshot,
            repl,
            script_name,
        } => {
            let repl_py = create_py_repl(py, repl, &script_name, &dc_registry)?;
            let either = snapshot.into_either_with_repl(repl_py.clone_ref(py));
            let snap = PyFutureSnapshot::from_deserialized(py, either, print_callback, dc_registry, script_name)?;
            Ok((snap, repl_py))
        }
    }
}

/// Creates a `Py<PyMontyRepl>` from deserialized REPL state.
fn create_py_repl(
    py: Python<'_>,
    repl: EitherRepl,
    script_name: &str,
    dc_registry: &DcRegistry,
) -> PyResult<Py<PyMontyRepl>> {
    let repl_obj = PyMontyRepl::from_deserialized(repl, script_name.to_owned(), dc_registry.clone_ref(py));
    Py::new(py, repl_obj)
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

/// Converts a `Py<PyTuple>` of Python args to `Vec<MontyObject>`.
fn convert_args_to_monty(py: Python<'_>, args: &Py<PyTuple>, dc_registry: &DcRegistry) -> PyResult<Vec<MontyObject>> {
    args.bind(py)
        .iter()
        .map(|item| py_to_monty(&item, dc_registry))
        .collect()
}

/// Converts a `Py<PyDict>` of Python kwargs to `Vec<(MontyObject, MontyObject)>`.
fn convert_kwargs_to_monty(
    py: Python<'_>,
    kwargs: &Py<PyDict>,
    dc_registry: &DcRegistry,
) -> PyResult<Vec<(MontyObject, MontyObject)>> {
    kwargs
        .bind(py)
        .iter()
        .map(|(k, v)| Ok((py_to_monty(&k, dc_registry)?, py_to_monty(&v, dc_registry)?)))
        .collect()
}

/// Converts `&[MontyObject]` to a Python tuple.
fn monty_objects_to_py_tuple(
    py: Python<'_>,
    objects: &[MontyObject],
    dc_registry: &DcRegistry,
) -> PyResult<Py<PyTuple>> {
    let items: Vec<Py<PyAny>> = objects
        .iter()
        .map(|item| monty_to_py(py, item, dc_registry))
        .collect::<PyResult<_>>()?;
    Ok(PyTuple::new(py, items)?.unbind())
}

/// Converts `&[(MontyObject, MontyObject)]` to a Python dict.
fn monty_pairs_to_py_dict(
    py: Python<'_>,
    pairs: &[(MontyObject, MontyObject)],
    dc_registry: &DcRegistry,
) -> PyResult<Py<PyDict>> {
    let dict = PyDict::new(py);
    for (k, v) in pairs {
        dict.set_item(monty_to_py(py, k, dc_registry)?, monty_to_py(py, v, dc_registry)?)?;
    }
    Ok(dict.unbind())
}

// ---------------------------------------------------------------------------
// Trait extensions on Either*Snapshot for REPL detection and state extraction
// ---------------------------------------------------------------------------

impl EitherFunctionSnapshot {
    /// Returns `true` if this snapshot is from a REPL `feed_start()` call.
    pub(crate) fn is_repl(&self) -> bool {
        matches!(
            self,
            Self::ReplNoLimitFn(..) | Self::ReplNoLimitOs(..) | Self::ReplLimitedFn(..) | Self::ReplLimitedOs(..)
        )
    }

    /// Extracts the REPL state from a REPL variant, taking it from the `Py<PyMontyRepl>`.
    ///
    /// This calls `take_repl()` on the owning `PyMontyRepl` to extract its internal state.
    pub(crate) fn take_repl_state(&self) -> PyResult<EitherRepl> {
        let (Self::ReplNoLimitFn(_, repl_owner)
        | Self::ReplNoLimitOs(_, repl_owner)
        | Self::ReplLimitedFn(_, repl_owner)
        | Self::ReplLimitedOs(_, repl_owner)) = self
        else {
            return Err(PyRuntimeError::new_err(
                "Cannot extract REPL state from a non-REPL snapshot",
            ));
        };
        repl_owner.get().take_repl()
    }
}

impl EitherLookupSnapshot {
    /// Returns `true` if this snapshot is from a REPL `feed_start()` call.
    pub(crate) fn is_repl(&self) -> bool {
        matches!(self, Self::ReplNoLimit(..) | Self::ReplLimited(..))
    }

    /// Extracts the REPL state from a REPL variant.
    pub(crate) fn take_repl_state(&self) -> PyResult<EitherRepl> {
        let (Self::ReplNoLimit(_, repl_owner) | Self::ReplLimited(_, repl_owner)) = self else {
            return Err(PyRuntimeError::new_err(
                "Cannot extract REPL state from a non-REPL snapshot",
            ));
        };
        repl_owner.get().take_repl()
    }
}

impl EitherFutureSnapshot {
    /// Returns `true` if this snapshot is from a REPL `feed_start()` call.
    pub(crate) fn is_repl(&self) -> bool {
        matches!(self, Self::ReplNoLimit(..) | Self::ReplLimited(..))
    }

    /// Extracts the REPL state from a REPL variant.
    pub(crate) fn take_repl_state(&self) -> PyResult<EitherRepl> {
        let (Self::ReplNoLimit(_, repl_owner) | Self::ReplLimited(_, repl_owner)) = self else {
            return Err(PyRuntimeError::new_err(
                "Cannot extract REPL state from a non-REPL snapshot",
            ));
        };
        repl_owner.get().take_repl()
    }
}
