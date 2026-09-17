//! Leaf conversions shared by both directions of the Python boundary: host
//! type objects, dates and times, file handles and callables. The arena walks
//! are `encode` (Python → sandbox) and `decode` (sandbox → Python).

use std::borrow::Cow;

use monty_types::{
    FileMode, MontyDateTime, MontyFileHandle, MontyNode, MontyTime, MontyTimeDelta, MontyTimeZone, MontyType,
    StringRepr,
};
use pyo3::{
    exceptions::{PyTypeError, PyValueError},
    intern,
    prelude::*,
    sync::PyOnceLock,
    types::{
        PyDateAccess, PyDateTime, PyDelta, PyDeltaAccess, PyList, PyModule, PyTime, PyTimeAccess, PyTuple, PyType,
        PyTzInfo, PyTzInfoAccess,
    },
};

/// Inverse of [`type_object_to_py`]: maps a host class passed *into* the sandbox
/// to the Monty [`MontyType`] it represents, so it round-trips instead of degrading to
/// a callable. Matches by type-object **identity**, not `__module__`/`__name__` —
/// the latter is spoofable and churns across Python versions (e.g. `pathlib` paths
/// report `pathlib._local` on 3.13). Every `pathlib` path class collapses to
/// [`MontyType::Path`]. Returns `None` for classes Monty does not model, which the
/// caller then represents as a function node.
pub(super) fn py_type_object_to_monty(ty: &Bound<'_, PyType>) -> PyResult<Option<MontyType>> {
    let py = ty.py();
    for (obj, t) in round_trip_type_table(py)? {
        if ty.is(obj) {
            return Ok(Some(t.clone()));
        }
    }
    // pathlib's concrete path classes (PurePath, PosixPath, …) all subclass
    // PurePath and collapse to one Monty path type.
    Ok(ty.is_subclass(get_pure_path(py)?)?.then_some(MontyType::Path))
}

/// Host type objects that round-trip into the sandbox, each paired with its Monty
/// [`MontyType`]. Built once and cached. Identities are taken from [`type_object_to_py`]
/// so the two directions stay in lock-step. [`MontyType::Path`] is handled separately
/// (by subclass check) since pathlib exposes several concrete path classes.
///
/// The whole table is built in one go, so an entry the host cannot resolve would
/// fail every lookup, not just its own — [`host_has_type`] keeps those out.
fn round_trip_type_table(py: Python<'_>) -> PyResult<&'static Vec<(Py<PyAny>, MontyType)>> {
    static TABLE: PyOnceLock<Vec<(Py<PyAny>, MontyType)>> = PyOnceLock::new();
    TABLE.get_or_try_init(py, || {
        [
            MontyType::NoneType,
            MontyType::Ellipsis,
            MontyType::Bool,
            MontyType::Int,
            MontyType::Float,
            MontyType::Str,
            MontyType::Bytes,
            MontyType::List,
            MontyType::Deque,
            MontyType::ListIterator,
            MontyType::CallableIterator,
            MontyType::ItertoolsCount,
            MontyType::ItertoolsRepeat,
            MontyType::Partial,
            MontyType::GenericAlias,
            MontyType::Union,
            MontyType::ItertoolsPairwise,
            MontyType::ItertoolsCompress,
            MontyType::ItertoolsIslice,
            MontyType::ItertoolsChain,
            MontyType::ItertoolsCycle,
            MontyType::ItertoolsTakeWhile,
            MontyType::ItertoolsDropWhile,
            MontyType::ItertoolsFilterFalse,
            MontyType::ItertoolsStarMap,
            MontyType::ItertoolsAccumulate,
            MontyType::ItertoolsBatched,
            MontyType::ItertoolsZipLongest,
            MontyType::ItertoolsCombinations,
            MontyType::ItertoolsCombinationsWithReplacement,
            MontyType::ItertoolsPermutations,
            MontyType::ItertoolsProduct,
            MontyType::ItertoolsGroupBy,
            MontyType::ItertoolsGrouper,
            MontyType::ItertoolsTee,
            MontyType::ItertoolsTeeDataObject,
            MontyType::Tuple,
            MontyType::Dict,
            MontyType::Set,
            MontyType::FrozenSet,
            MontyType::Range,
            MontyType::Slice,
            MontyType::Type,
            MontyType::Property,
            MontyType::Date,
            MontyType::DateTime,
            MontyType::Time,
            MontyType::TimeDelta,
            MontyType::TimeZone,
            MontyType::RePattern,
            MontyType::ReMatch,
            MontyType::TextIOWrapper,
            MontyType::BufferedReader,
            MontyType::BufferedWriter,
            MontyType::BufferedRandom,
            MontyType::SpecialForm,
        ]
        .into_iter()
        .filter(|t| host_has_type(py, t))
        .map(|t| Ok((type_object_to_py(py, &t)?, t)))
        .collect()
    })
}

/// Whether this host's Python is new enough to define `t`'s type object.
///
/// A type the host does not have can never be the class being looked up, so it
/// is left out of [`round_trip_type_table`] rather than failing the build of it.
/// Outbound it is the guard in [`type_object_to_py`], which has a real value to
/// reject rather than a table entry to skip.
/// Runtime version check (not `cfg!(Py_3_12)`): this crate has no
/// pyo3-build-config build script, so the version cfgs don't exist.
fn host_has_type(py: Python<'_>, t: &MontyType) -> bool {
    match t {
        // `itertools.batched` is 3.12+, below the packages' 3.10 floor.
        MontyType::ItertoolsBatched => py.version_info() >= (3, 12),
        _ => true,
    }
}

/// Resolves a builtin function's host object from the name Monty renders it as.
///
/// Nearly every name is a plain `builtins` attribute, but `object.__setattr__`
/// is dotted — it lives on `object`, not on the module — so the name is walked
/// segment by segment rather than looked up whole.
pub(super) fn builtin_function_to_py(py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
    let mut obj: Py<PyAny> = import_builtins(py)?.clone_ref(py).into_any();
    for segment in name.split('.') {
        obj = obj.getattr(py, segment)?;
    }
    Ok(obj)
}

pub fn import_builtins(py: Python<'_>) -> PyResult<&Py<PyModule>> {
    static BUILTINS: PyOnceLock<Py<PyModule>> = PyOnceLock::new();

    BUILTINS.get_or_try_init(py, || py.import("builtins").map(Bound::unbind))
}

/// Reconstructs the host Python *type object* for a Monty [`MontyType`] crossing the
/// boundary as a value (e.g. sandbox code passing `type(Path('/x'))` to a host call).
///
/// Genuine builtins resolve from `builtins`; modeled stdlib types resolve from their
/// real defining module (the `Path` class maps to `PurePosixPath`, like its instances).
/// The import path can differ from [`MontyType`]'s `Display` (io types show `_io.*` but
/// live in `io`). Unmodeled types fall through to `builtins` and raise `AttributeError`.
/// Each modeled type's host class is cached in its own `PyOnceLock` (imported once).
pub(super) fn type_object_to_py(py: Python<'_>, t: &MontyType) -> PyResult<Py<PyAny>> {
    // A type this host's Python is too old to define has no object to hand back.
    // Say which type that was, rather than leaving the arm's import to raise a
    // bare `AttributeError` naming neither. Same predicate as the filter in
    // `round_trip_type_table`, so both directions agree on what this host holds.
    if !host_has_type(py, t) {
        return Err(PyTypeError::new_err(format!(
            "Cannot convert {t} to a host type: this Python does not define it"
        )));
    }

    // Each expansion gets a distinct hygienic `LOCK` static, so every arm caches
    // its own resolved type object. `PyOnceLock::import` imports + getattrs once.
    macro_rules! cached {
        ($module:literal, $name:literal) => {{
            static LOCK: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
            LOCK.import(py, $module, $name).map(|b| b.clone().unbind())
        }};
    }
    match t {
        MontyType::Date => cached!("datetime", "date"),
        MontyType::DateTime => cached!("datetime", "datetime"),
        MontyType::Time => cached!("datetime", "time"),
        MontyType::Deque => cached!("collections", "deque"),
        MontyType::TimeDelta => cached!("datetime", "timedelta"),
        MontyType::TimeZone => cached!("datetime", "timezone"),
        MontyType::ListIterator => get_list_iterator_type(py).map(|b| b.clone().unbind()),
        MontyType::CallableIterator => get_callable_iterator_type(py).map(|b| b.clone().unbind()),
        MontyType::ItertoolsCount => cached!("itertools", "count"),
        MontyType::ItertoolsRepeat => cached!("itertools", "repeat"),
        MontyType::Partial => cached!("functools", "partial"),
        MontyType::GenericAlias => cached!("types", "GenericAlias"),
        // `types.UnionType` is the type of `int | None` on every supported host;
        // on 3.14+ it is the same object as `typing.Union`.
        MontyType::Union => cached!("types", "UnionType"),
        MontyType::ItertoolsPairwise => cached!("itertools", "pairwise"),
        MontyType::ItertoolsCompress => cached!("itertools", "compress"),
        MontyType::ItertoolsIslice => cached!("itertools", "islice"),
        MontyType::ItertoolsChain => cached!("itertools", "chain"),
        MontyType::ItertoolsCycle => cached!("itertools", "cycle"),
        MontyType::ItertoolsTakeWhile => cached!("itertools", "takewhile"),
        MontyType::ItertoolsDropWhile => cached!("itertools", "dropwhile"),
        MontyType::ItertoolsFilterFalse => cached!("itertools", "filterfalse"),
        MontyType::ItertoolsStarMap => cached!("itertools", "starmap"),
        MontyType::ItertoolsAccumulate => cached!("itertools", "accumulate"),
        MontyType::ItertoolsBatched => cached!("itertools", "batched"),
        MontyType::ItertoolsZipLongest => cached!("itertools", "zip_longest"),
        MontyType::ItertoolsCombinations => cached!("itertools", "combinations"),
        MontyType::ItertoolsCombinationsWithReplacement => cached!("itertools", "combinations_with_replacement"),
        MontyType::ItertoolsPermutations => cached!("itertools", "permutations"),
        MontyType::ItertoolsProduct => cached!("itertools", "product"),
        MontyType::ItertoolsGroupBy => cached!("itertools", "groupby"),
        MontyType::ItertoolsGrouper => cached!("itertools", "_grouper"),
        MontyType::ItertoolsTee => cached!("itertools", "_tee"),
        MontyType::ItertoolsTeeDataObject => cached!("itertools", "_tee_dataobject"),
        // Consistent with the Path *instance* arm, which marshals as PurePosixPath
        // and is instantiable on every host OS (unlike PosixPath on Windows).
        MontyType::Path => get_pure_posix_path(py).map(|b| b.clone().unbind()),
        MontyType::RePattern => cached!("re", "Pattern"),
        MontyType::ReMatch => cached!("re", "Match"),
        MontyType::TextIOWrapper => cached!("io", "TextIOWrapper"),
        MontyType::BufferedReader => cached!("io", "BufferedReader"),
        MontyType::BufferedWriter => cached!("io", "BufferedWriter"),
        MontyType::BufferedRandom => cached!("io", "BufferedRandom"),
        MontyType::SpecialForm => cached!("typing", "_SpecialForm"),
        MontyType::Field => cached!("dataclasses", "Field"),
        // `NoneType` and `ellipsis` aren't `builtins` attributes; take them from
        // the singletons (`type(None)` / `type(...)`).
        MontyType::NoneType => Ok(py.None().bind(py).get_type().into_any().unbind()),
        MontyType::Ellipsis => Ok(py.Ellipsis().bind(py).get_type().into_any().unbind()),
        _ => import_builtins(py)?.getattr(py, t.to_string()),
    }
}

/// Returns CPython's private `list_iterator` type without relying on a module attribute.
fn get_list_iterator_type(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static TYPE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
    TYPE.get_or_try_init(py, || Ok(PyList::empty(py).try_iter()?.get_type().into_any().unbind()))
        .map(|ty| ty.bind(py))
}

/// Returns CPython's private `callable_iterator` type, which — like
/// `list_iterator` — is not reachable as a `builtins` attribute, so it is taken
/// from the type of a throwaway two-argument `iter()`.
fn get_callable_iterator_type(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static TYPE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();
    TYPE.get_or_try_init(py, || {
        // `iter(callable, sentinel)` does not call `callable` until advanced, and
        // this iterator never is — so any callable serves, and a builtin avoids
        // compiling a throwaway lambda.
        let callable = import_builtins(py)?.getattr(py, "id")?;
        let iterator = import_builtins(py)?.getattr(py, "iter")?.call1(py, (callable, 0))?;
        Ok(iterator.bind(py).get_type().into_any().unbind())
    })
    .map(|ty| ty.bind(py))
}

/// Converts a native Python `datetime.timedelta` to Monty's carrier representation.
pub(super) fn py_timedelta_to_monty(delta: &Bound<'_, PyDelta>) -> MontyTimeDelta {
    MontyTimeDelta {
        days: delta.get_days(),
        seconds: delta.get_seconds(),
        microseconds: delta.get_microseconds(),
    }
}

/// Converts a Monty timezone payload to a native Python `datetime.timezone`.
pub(super) fn monty_timezone_to_py(py: Python<'_>, timezone: &MontyTimeZone) -> PyResult<Py<PyAny>> {
    if timezone.offset_seconds == 0 && timezone.name.is_none() {
        return Ok(PyTzInfo::utc(py)?.to_owned().into_any().unbind());
    }

    let offset = PyDelta::new(py, 0, timezone.offset_seconds, 0, true)?;
    match timezone.name.as_deref() {
        None => PyTzInfo::fixed_offset(py, offset)
            .map(Bound::into_any)
            .map(Bound::unbind),
        Some(name) => get_datetime_timezone_type(py)?.call1((offset, name)).map(Bound::unbind),
    }
}

/// Converts a native Python `datetime.timezone` to Monty's carrier representation.
///
/// `timezone.__getinitargs__()` preserves whether the original Python object was
/// created with just an offset or with an explicit custom name, which is
/// important for Monty's repr/equality behavior.
pub(super) fn py_timezone_to_monty(obj: &Bound<'_, PyAny>) -> PyResult<MontyTimeZone> {
    if obj.is(get_datetime_timezone_utc(obj.py())?) {
        return Ok(MontyTimeZone {
            offset_seconds: 0,
            name: None,
        });
    }

    let init_args = obj.call_method0(intern!(obj.py(), "__getinitargs__"))?;
    let init_args = init_args.cast::<PyTuple>()?;

    Ok(MontyTimeZone {
        offset_seconds: timezone_offset_seconds(&py_timedelta_to_monty(
            &init_args.get_item(0)?.cast_into::<PyDelta>()?,
        ))?,
        name: init_args.get_item(1).and_then(|n| n.extract::<String>()).ok(),
    })
}

/// Converts a Monty time payload to a native Python `datetime.time`.
///
/// A name with no offset cannot be built: `datetime.timezone` has no such form,
/// and the wire rejects the pair, so it can only come from a hand-built value.
pub(super) fn monty_time_to_py(py: Python<'_>, time: &MontyTime) -> PyResult<Py<PyAny>> {
    let tzinfo_obj = match (time.offset_seconds, &time.timezone_name) {
        (None, None) => None,
        (Some(offset_seconds), timezone_name) => Some(monty_timezone_to_py(
            py,
            &MontyTimeZone {
                offset_seconds,
                name: timezone_name.clone(),
            },
        )?),
        (None, Some(_)) => {
            return Err(PyTypeError::new_err("invalid Monty time: timezone name without offset"));
        }
    };
    let tzinfo = tzinfo_obj
        .as_ref()
        .map(|obj| obj.bind(py).cast::<PyTzInfo>())
        .transpose()?;
    PyTime::new_with_fold(
        py,
        time.hour,
        time.minute,
        time.second,
        time.microsecond,
        tzinfo,
        time.fold != 0,
    )
    .map(Bound::into_any)
    .map(Bound::unbind)
}

/// Converts a Monty datetime payload to a native Python `datetime.datetime`.
pub(super) fn monty_datetime_to_py(py: Python<'_>, datetime: &MontyDateTime) -> PyResult<Py<PyAny>> {
    match (datetime.offset_seconds, &datetime.timezone_name) {
        (None, None) => PyDateTime::new(
            py,
            datetime.year,
            datetime.month,
            datetime.day,
            datetime.hour,
            datetime.minute,
            datetime.second,
            datetime.microsecond,
            None,
        )
        .map(Bound::into_any)
        .map(Bound::unbind),
        (Some(offset_seconds), timezone_name) => {
            let tzinfo_obj = monty_timezone_to_py(
                py,
                &MontyTimeZone {
                    offset_seconds,
                    name: timezone_name.clone(),
                },
            )?;
            let tzinfo = tzinfo_obj.bind(py).cast::<PyTzInfo>()?;
            PyDateTime::new(
                py,
                datetime.year,
                datetime.month,
                datetime.day,
                datetime.hour,
                datetime.minute,
                datetime.second,
                datetime.microsecond,
                Some(tzinfo),
            )
            .map(Bound::into_any)
            .map(Bound::unbind)
        }
        (None, Some(_)) => Err(PyTypeError::new_err(
            "invalid Monty datetime: timezone name without offset",
        )),
    }
}

/// Converts a native Python `datetime.datetime` to Monty's carrier representation.
///
/// For `datetime.timezone` tzinfo objects, uses `__getinitargs__()` to preserve
/// the explicit-vs-auto-generated name distinction. For other tzinfo types
/// (e.g. `zoneinfo.ZoneInfo`), falls back to the standard `utcoffset()`/`tzname()`
/// protocol on the datetime itself.
pub(super) fn py_datetime_to_monty(datetime: &Bound<'_, PyDateTime>) -> PyResult<MontyNode> {
    let (offset_seconds, timezone_name) = if let Some(tzinfo) = datetime.get_tzinfo() {
        if tzinfo.is_instance(get_datetime_timezone_type(tzinfo.py())?)? {
            // datetime.timezone — use __getinitargs__ for round-trip fidelity
            let timezone = py_timezone_to_monty(&tzinfo)?;
            (Some(timezone.offset_seconds), timezone.name)
        } else {
            // Other tzinfo (e.g. zoneinfo.ZoneInfo) — use standard protocol
            py_tzinfo_via_utcoffset(datetime, &tzinfo)?
        }
    } else {
        (None, None)
    };

    Ok(MontyNode::DateTime(MontyDateTime {
        year: datetime.get_year(),
        month: datetime.get_month(),
        day: datetime.get_day(),
        hour: datetime.get_hour(),
        minute: datetime.get_minute(),
        second: datetime.get_second(),
        microsecond: datetime.get_microsecond(),
        offset_seconds,
        timezone_name,
    }))
}

/// Converts a host `datetime.time`, preserving `fold` and its timezone.
///
/// A naive time has no instant for `utcoffset()` to resolve against, so unlike
/// `datetime` only a `datetime.timezone` is accepted: CPython passes `None` to
/// `tzinfo.utcoffset(None)`, and a zone that needs a date (`ZoneInfo`) returns
/// `None` there rather than a usable offset.
pub(super) fn py_time_to_monty(time: &Bound<'_, PyTime>) -> PyResult<MontyNode> {
    let (offset_seconds, timezone_name) = match time.get_tzinfo() {
        Some(tzinfo) if tzinfo.is_instance(get_datetime_timezone_type(tzinfo.py())?)? => {
            let timezone = py_timezone_to_monty(&tzinfo)?;
            (Some(timezone.offset_seconds), timezone.name)
        }
        Some(tzinfo) => {
            return Err(PyTypeError::new_err(format!(
                "cannot convert datetime.time with tzinfo of type '{}' to a Monty value",
                tzinfo.get_type().name()?
            )));
        }
        None => (None, None),
    };

    Ok(MontyNode::Time(MontyTime {
        hour: time.get_hour(),
        minute: time.get_minute(),
        second: time.get_second(),
        microsecond: time.get_microsecond(),
        offset_seconds,
        timezone_name,
        fold: u8::from(time.get_fold()),
    }))
}

/// Extracts timezone offset and name from a non-`datetime.timezone` tzinfo
/// (e.g. `zoneinfo.ZoneInfo`) using the standard `utcoffset()`/`tzname()` protocol.
///
/// Unlike `__getinitargs__()`, this always produces a name (since IANA timezones
/// always have one), so the name is stored as `Some(...)`.
fn py_tzinfo_via_utcoffset(
    datetime: &Bound<'_, PyDateTime>,
    tzinfo: &Bound<'_, PyAny>,
) -> PyResult<(Option<i32>, Option<String>)> {
    let py = tzinfo.py();
    let utcoffset = tzinfo
        .call_method1(intern!(py, "utcoffset"), (datetime,))?
        .cast_into::<PyDelta>()?;
    let offset = py_timedelta_to_monty(&utcoffset);
    let offset_seconds = timezone_offset_seconds(&offset)?;

    let name = tzinfo
        .call_method1(intern!(py, "tzname"), (datetime,))?
        .extract::<Option<String>>()?;

    Ok((Some(offset_seconds), name))
}

/// Converts a MontyTimeDelta to exact whole seconds for timezone offsets.
fn timezone_offset_seconds(delta: &MontyTimeDelta) -> PyResult<i32> {
    if delta.microseconds != 0 {
        return Err(PyTypeError::new_err(
            "datetime.timezone offset must be an exact number of whole seconds",
        ));
    }
    let total_seconds = i64::from(delta.days)
        .checked_mul(86_400)
        .and_then(|days| days.checked_add(i64::from(delta.seconds)))
        .ok_or_else(|| PyTypeError::new_err("datetime.timezone offset is out of range"))?;
    i32::try_from(total_seconds).map_err(|_| PyTypeError::new_err("datetime.timezone offset is out of range"))
}

/// Returns the Python `datetime.timezone` type object.
pub(super) fn get_datetime_timezone_type(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static TIMEZONE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    TIMEZONE.import(py, "datetime", "timezone")
}

/// Returns Python's `datetime.timezone.utc` singleton.
fn get_datetime_timezone_utc(py: Python<'_>) -> PyResult<&Py<PyAny>> {
    static TIMEZONE_UTC: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    TIMEZONE_UTC.get_or_try_init(py, || {
        get_datetime_timezone_type(py)?
            .getattr(intern!(py, "utc"))
            .map(Bound::unbind)
    })
}

/// Cached import of `collections.namedtuple` function.
pub(super) fn get_namedtuple(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static NAMEDTUPLE: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    NAMEDTUPLE.import(py, "collections", "namedtuple")
}

/// Cached import of `pathlib.PurePosixPath` class.
pub(super) fn get_pure_posix_path(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static PUREPOSIX: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    PUREPOSIX.import(py, "pathlib", "PurePosixPath")
}

/// Cached import of `pathlib.PurePath` — the common base of every path class,
/// used to recognise any path type passed into the sandbox.
fn get_pure_path(py: Python<'_>) -> PyResult<&Bound<'_, PyAny>> {
    static PUREPATH: PyOnceLock<Py<PyAny>> = PyOnceLock::new();

    PUREPATH.import(py, "pathlib", "PurePath")
}

/// Host-side mirror of a [`MontyFileHandle`] value: a thin PyO3 wrapper holding
/// the same [`MontyFileHandle`] value the interpreter does.
///
/// A Python host sees one when a sandbox-opened file flows back across the
/// boundary (e.g. the return of an `Open` OS callback, or the first argument to
/// a `read`/`write` callback). It is a plain data holder — the runtime
/// guarantees the host never owns a live OS file descriptor for a Monty file,
/// so there is nothing to clean up.
///
/// Fields are read-only via getters; `binary`/`readable`/`writable` are derived
/// from the underlying [`FileMode`] on demand.
#[pyclass(name = "MontyFileHandle", module = "pydantic_monty", frozen)]
pub struct PyMontyFileHandle(MontyFileHandle);

impl PyMontyFileHandle {
    /// Wraps an existing [`MontyFileHandle`] for surfacing back to Python,
    /// reusing the interpreter's value instead of repacking its fields.
    pub(crate) fn from_inner(inner: MontyFileHandle) -> Self {
        Self(inner)
    }

    /// The file the handle stands for.
    pub(super) fn inner(&self) -> &MontyFileHandle {
        &self.0
    }
}

#[pymethods]
impl PyMontyFileHandle {
    /// Constructs a `MontyFileHandle` from Python.
    ///
    /// `mode` is parsed via [`FileMode::from_str`] and rewritten to its
    /// canonical form, so `MontyFileHandle('/x', 'rt').mode == 'r'`. This
    /// is the path Python callbacks use to return file handles from the
    /// `Open` OS function.
    #[new]
    #[pyo3(signature = (path, mode, *, position = 0))]
    fn py_new(path: String, mode: &str, position: u64) -> PyResult<Self> {
        let mode: FileMode = mode
            .parse()
            .map_err(|e: Cow<'static, str>| PyValueError::new_err(e.to_string()))?;
        Ok(Self::from_inner(MontyFileHandle { path, mode, position }))
    }

    /// Virtual sandbox path of the open file. Always POSIX-style; never a host path.
    #[getter]
    fn path(&self) -> &str {
        &self.0.path
    }

    /// Canonical `open()` mode string (e.g. `'r'`, `'rb'`, `'w+'`).
    #[getter]
    fn mode(&self) -> &'static str {
        self.0.mode.as_str()
    }

    /// Current position for sized/line/seek operations: char index in text
    /// mode, byte index in binary mode. `0` for freshly opened files.
    #[getter]
    fn position(&self) -> u64 {
        self.0.position
    }

    /// `True` if the underlying mode opens the file in binary form (`'rb'`, `'wb'`, …).
    #[getter]
    fn binary(&self) -> bool {
        self.0.mode.is_binary()
    }

    /// `True` if the file's mode permits `read()`.
    #[getter]
    fn readable(&self) -> bool {
        self.0.mode.readable()
    }

    /// `True` if the file's mode permits `write()`.
    #[getter]
    fn writable(&self) -> bool {
        self.0.mode.writable()
    }

    fn __repr__(&self) -> String {
        format!(
            "MontyFileHandle(path={}, mode={})",
            StringRepr(&self.0.path),
            StringRepr(self.0.mode.as_str())
        )
    }
}

pub fn get_name(f: &Bound<'_, PyAny>) -> String {
    f.getattr(intern!(f.py(), "__name__"))
        .and_then(|n| n.extract::<String>())
        .unwrap_or_else(|_| "<unknown>".to_string())
}

/// get the `__doc__` attribute from a (hopefully) function
pub fn get_docstring(f: &Bound<'_, PyAny>) -> Option<String> {
    f.getattr(intern!(f.py(), "__doc__"))
        .and_then(|d| d.extract::<String>())
        .ok()
}
