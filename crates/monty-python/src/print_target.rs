//! Routing destination for Monty `print()` output.
//!
//! Python callers pass a `print_callback` argument which may be:
//!
//! - `None` — print fragments go to the process stdout (default).
//! - A callable `(stream, text) -> None` — each fragment is forwarded to the
//!   callback. Used e.g. to tee output to a logger.
//! - A `CollectStreams()` instance — fragments accumulate into a shared buffer
//!   of `(stream, text)` tuples exposed via `CollectStreams.output`.
//! - A `CollectString()` instance — fragments accumulate into a shared flat
//!   `String` exposed via `CollectString.output`.
//!
//! This module encapsulates that dispatch. The rest of the bindings thread a
//! [`PrintTarget`] value through `start`/`resume`/`run`/`run_async`, while the
//! collector objects themselves remain the single public place that exposes the
//! captured output.

use std::{
    borrow::Cow,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use monty::{MontyException, PrintStream, PrintWriter, PrintWriterCallback};
use pyo3::{
    PyRef,
    exceptions::PyTypeError,
    intern,
    prelude::*,
    types::{PyList, PyString},
};

use crate::exceptions::exc_py_to_monty;

/// Shared buffer for the `CollectStreams` mode.
///
/// The `Arc<Mutex<..>>` wrapper lets a single collector keep accumulating
/// across `start()` / `resume()` / async / snapshot-load boundaries while still
/// allowing read access from Python between transitions.
type CollectStreamsBuffer = Arc<Mutex<Vec<(PrintStream, String)>>>;

/// Shared buffer for the `CollectString` mode.
///
/// This follows the same sharing scheme as [`CollectStreamsBuffer`], but stores
/// a flat concatenated string instead of labelled stream fragments.
type CollectStringBuffer = Arc<Mutex<String>>;

/// Python collector that records printed fragments as `(stream, text)` tuples.
///
/// Pass `CollectStreams()` as `print_callback` to share one collector across an
/// entire run or snapshot chain. Reading `.output` clones the current buffer
/// without draining it, so callers can inspect intermediate state and continue
/// accumulating into the same collector.
#[pyclass(name = "CollectStreams", module = "pydantic_monty", frozen)]
#[derive(Debug, Default)]
pub struct PyCollectStreams {
    buffer: CollectStreamsBuffer,
}

#[pymethods]
impl PyCollectStreams {
    /// Creates an empty stream collector.
    #[new]
    fn new() -> Self {
        Self::default()
    }

    /// Returns the collected `(stream, text)` tuples so far.
    #[getter]
    fn output<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        PyList::new(
            py,
            self.buffer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .map(|(stream, text)| {
                    let label = match stream {
                        PrintStream::Stdout => intern!(py, "stdout"),
                        PrintStream::Stderr => intern!(py, "stderr"),
                    };
                    (label, text.as_str())
                }),
        )
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!("CollectStreams(output={})", self.output(py)?.repr()?))
    }
}

impl PyCollectStreams {
    /// Returns a cloneable handle to the shared collector buffer.
    fn buffer(&self) -> CollectStreamsBuffer {
        self.buffer.clone()
    }
}

/// Python collector that records printed fragments into one concatenated string.
///
/// Pass `CollectString()` as `print_callback` to accumulate raw printed text
/// while still letting the corresponding run or snapshot return its ordinary
/// execution value.
#[pyclass(name = "CollectString", module = "pydantic_monty", frozen)]
#[derive(Debug, Default)]
pub struct PyCollectString {
    buffer: CollectStringBuffer,
}

#[pymethods]
impl PyCollectString {
    /// Creates an empty string collector.
    #[new]
    fn new() -> Self {
        Self::default()
    }

    /// Returns the collected text so far.
    #[getter]
    fn output<'py>(&self, py: Python<'py>) -> Bound<'py, PyString> {
        let guard = self.buffer.lock().unwrap_or_else(PoisonError::into_inner);
        PyString::new(py, guard.as_str())
    }

    fn __repr__(&self, py: Python<'_>) -> PyResult<String> {
        Ok(format!("CollectString(output={})", self.output(py).repr()?))
    }
}

impl PyCollectString {
    /// Returns a cloneable handle to the shared collector buffer.
    fn buffer(&self) -> CollectStringBuffer {
        self.buffer.clone()
    }
}

/// Destination for Monty `print()` output.
///
/// The variant is chosen once from the Python `print_callback` argument (via
/// [`PrintTarget::from_py`]) and threaded through the execution chain. It is
/// not invoked directly — call [`PrintTarget::with_writer`] to build a
/// `PrintWriter` on demand for each VM transition.
///
/// # Foot-guns
///
/// - The `CollectStreams` / `CollectString` variants hold an `Arc`; cloning is
///   cheap but **shares** the buffer. Use [`PrintTarget::clone_handle`] /
///   [`clone_handle_detached`](Self::clone_handle_detached) instead of `Clone`
///   so the intent is explicit.
#[derive(Debug, Default)]
pub(crate) enum PrintTarget {
    /// Print goes to process stdout — the default when no `print_callback` is set.
    #[default]
    Stdout,
    /// Each fragment is forwarded to a Python callable as `(stream_name, text)`.
    Callback(Py<PyAny>),
    /// Each fragment accumulates into a shared buffer of `(stream, text)`
    /// tuples, surfaced as `list[tuple[str, str]]` in Python.
    CollectStreams(CollectStreamsBuffer),
    /// Each fragment is appended to a shared flat `String`, surfaced as `str`
    /// in Python — no stream labels, emit order preserved.
    CollectString(CollectStringBuffer),
}

impl PrintTarget {
    /// Parses a Python `print_callback` argument into a `PrintTarget`.
    ///
    /// Accepts `None`, a callable, `CollectStreams()`, or `CollectString()`.
    /// Any other value is a `TypeError` so mistakes surface eagerly rather
    /// than during execution.
    pub fn from_py(value: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let Some(obj) = value else {
            return Ok(Self::Stdout);
        };
        if let Ok(collector) = obj.extract::<PyRef<'_, PyCollectStreams>>() {
            Ok(Self::CollectStreams(collector.buffer()))
        } else if let Ok(collector) = obj.extract::<PyRef<'_, PyCollectString>>() {
            Ok(Self::CollectString(collector.buffer()))
        } else if obj.is_callable() {
            Ok(Self::Callback(obj.clone().unbind()))
        } else {
            Err(PyTypeError::new_err(
                "print_callback must be a callable, CollectStreams(), CollectString(), or None",
            ))
        }
    }

    /// Returns a fresh `PrintTarget` that targets the same sink as `self`.
    ///
    /// - `Stdout` → `Stdout` (nothing to share).
    /// - `Callback` → clones the `Py<PyAny>` reference using the provided GIL
    ///   token.
    /// - `CollectStreams` / `CollectString` → clones the `Arc`, so the new
    ///   target **writes into the same buffer**. This is the desired behavior
    ///   for threading the target through `start`/`resume` chains and into
    ///   `spawn_blocking` workers.
    ///
    /// Used instead of `Clone` to make the share-vs-copy intent explicit.
    /// Callers without a `Python` token in scope should use
    /// [`clone_handle_detached`](Self::clone_handle_detached) instead.
    pub fn clone_handle(&self, py: Python<'_>) -> Self {
        match self {
            Self::Stdout => Self::Stdout,
            Self::Callback(cb) => Self::Callback(cb.clone_ref(py)),
            Self::CollectStreams(arc) => Self::CollectStreams(arc.clone()),
            Self::CollectString(arc) => Self::CollectString(arc.clone()),
        }
    }

    /// Detached variant of [`clone_handle`](Self::clone_handle) for callers
    /// running without the GIL held (e.g. inside an `async move` block or a
    /// `spawn_blocking` worker about to hand the clone to another thread).
    ///
    /// Acquires the GIL internally only when the `Callback` variant actually
    /// needs it; `Stdout` and the two collect variants skip the acquisition
    /// entirely.
    pub fn clone_handle_detached(&self) -> Self {
        match self {
            Self::Stdout => Self::Stdout,
            Self::Callback(_) => Python::attach(|py| self.clone_handle(py)),
            Self::CollectStreams(arc) => Self::CollectStreams(arc.clone()),
            Self::CollectString(arc) => Self::CollectString(arc.clone()),
        }
    }

    /// Builds a `PrintWriter` for a single VM transition and invokes `f` with it.
    ///
    /// The writer borrows from this target for the duration of `f`, so the
    /// closure shape keeps lifetimes sound. For the collect variants, the
    /// internal mutex is held for the entirety of `f` — that is fine because a
    /// single VM transition is synchronous and Python only inspects collectors
    /// between transitions.
    pub fn with_writer<R>(&self, f: impl FnOnce(PrintWriter<'_>) -> R) -> R {
        let mut storage = self.storage();
        f(storage.writer())
    }

    /// Allocates writer-local storage (callback wrapper, mutex guard) that can
    /// back a `PrintWriter` produced by [`PrintStorage::writer`].
    ///
    /// Use this instead of [`with_writer`] when a caller needs to hold the
    /// writer across multiple VM transitions and reborrow it for each step
    /// (e.g. the synchronous dispatch loop in `Monty.run`). The storage keeps
    /// the `CallbackStringPrint` / `MutexGuard` alive while the writer pointer
    /// remains valid.
    pub fn storage(&self) -> PrintStorage<'_> {
        match self {
            Self::Stdout => PrintStorage::Stdout,
            // Borrow the callback rather than clone it — the storage's lifetime
            // is bounded by the target, so there is no need to bump the Py ref
            // count per VM transition (which would require reacquiring the GIL
            // inside `py.detach`).
            Self::Callback(cb) => PrintStorage::Callback(CallbackStringPrint(cb)),
            Self::CollectStreams(arc) => {
                PrintStorage::CollectStreams(arc.lock().unwrap_or_else(PoisonError::into_inner))
            }
            Self::CollectString(arc) => PrintStorage::CollectString(arc.lock().unwrap_or_else(PoisonError::into_inner)),
        }
    }
}

/// Live writer storage — owns the per-call backing (mutex guard, callback
/// wrapper) that a `PrintWriter` points into.
///
/// Produced by [`PrintTarget::storage`] and consumed by repeatedly calling
/// [`PrintStorage::writer`] (which hands out a fresh `PrintWriter` each time
/// with lifetime tied to this storage). This two-step split exists because
/// the `PrintWriter::Collect*` variants need `&mut` access to a locked buffer,
/// and `PrintWriter::Callback` needs `&mut` access to a `CallbackStringPrint`
/// value — both of which must outlive the writer.
pub(crate) enum PrintStorage<'a> {
    /// No-op storage — the writer just targets stdout.
    Stdout,
    /// Borrowed callback wrapper — points at the `Py<PyAny>` owned by the
    /// parent `PrintTarget::Callback` variant.
    Callback(CallbackStringPrint<'a>),
    /// Live `MutexGuard` over the shared streams buffer, held for as long as
    /// this storage exists.
    CollectStreams(MutexGuard<'a, Vec<(PrintStream, String)>>),
    /// Live `MutexGuard` over the shared string buffer, held for as long as
    /// this storage exists.
    CollectString(MutexGuard<'a, String>),
}

impl PrintStorage<'_> {
    /// Returns a `PrintWriter` backed by this storage.
    ///
    /// The returned writer borrows from `self`; call repeatedly (including via
    /// `PrintWriter::reborrow`) to get fresh writers with progressively shorter
    /// lifetimes, without dropping the underlying storage.
    pub fn writer(&mut self) -> PrintWriter<'_> {
        match self {
            Self::Stdout => PrintWriter::Stdout,
            Self::Callback(cb) => PrintWriter::Callback(cb),
            Self::CollectStreams(guard) => PrintWriter::CollectStreams(guard),
            Self::CollectString(guard) => PrintWriter::CollectString(guard),
        }
    }
}

/// `PrintWriterCallback` adaptor that forwards each fragment to a Python callable.
///
/// Borrows the `Py<PyAny>` from the parent `PrintTarget` rather than cloning
/// it; this avoids reacquiring the GIL on every VM transition just to bump the
/// reference count. `Py<PyAny>` is `Send + Sync`, so the shared reference is
/// safe to move across `py.detach` / `spawn_blocking` boundaries. The GIL is
/// re-acquired once per actual print fragment inside the trait methods —
/// which is unavoidable, since that is when we call into Python.
#[derive(Debug)]
pub(crate) struct CallbackStringPrint<'a>(&'a Py<PyAny>);

impl PrintWriterCallback for CallbackStringPrint<'_> {
    fn stdout_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        Python::attach(|py| {
            self.0.bind(py).call1(("stdout", output.as_ref()))?;
            Ok::<_, PyErr>(())
        })
        .map_err(|e| Python::attach(|py| exc_py_to_monty(py, &e)))
    }

    fn stdout_push(&mut self, end: char) -> Result<(), MontyException> {
        // Encode the character into a stack buffer to avoid allocating a
        // fresh `String` for each separator / terminator that `print()` emits.
        let mut buf = [0u8; 4];
        let end_str: &str = end.encode_utf8(&mut buf);
        Python::attach(|py| {
            self.0.bind(py).call1(("stdout", end_str))?;
            Ok::<_, PyErr>(())
        })
        .map_err(|e| Python::attach(|py| exc_py_to_monty(py, &e)))
    }
}
