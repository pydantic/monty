//! Routing destination for Monty `print()` output.
//!
//! Python callers pass a `print_callback` argument which may be:
//!
//! - `None` — print fragments go to the process stdout (default).
//! - A callable `(stream, text) -> None` — each fragment is forwarded to the
//!   callback. Used e.g. to tee output to a logger.
//! - The string `'collect'` — fragments accumulate into an internal buffer of
//!   `(stream, text)` tuples. The buffer is surfaced via `MontyComplete.print_output`
//!   on success and `MontyRuntimeError.print_output` on failure, and is live-
//!   visible on `FunctionSnapshot` / `NameLookupSnapshot` / `FutureSnapshot` for
//!   inspection mid-run.
//!
//! This module encapsulates that dispatch. The rest of the bindings thread a
//! [`PrintTarget`] value through `start`/`resume`/`run`/`run_async` in place of
//! the previous `Option<Py<PyAny>>` and delegate writer construction here.

use std::{
    borrow::Cow,
    mem,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use monty::{MontyException, PrintStream, PrintWriter, PrintWriterCallback};
use pyo3::{
    exceptions::PyTypeError,
    intern,
    prelude::*,
    types::{PyList, PyString, PyTuple},
};

use crate::exceptions::exc_py_to_monty;

/// Shared collect buffer — wrapped in an `Arc<Mutex<..>>` so the buffer survives
/// across `start`/`resume` snapshot boundaries (the snapshot holds one handle,
/// the VM call locks it for the duration of a transition) and can also be
/// observed concurrently via `snapshot_py` for live snapshot inspection.
type CollectBuffer = Arc<Mutex<Vec<(PrintStream, String)>>>;

/// Destination for Monty `print()` output.
///
/// The variant is chosen once from the Python `print_callback` argument (via
/// [`PrintTarget::from_py`]) and threaded through the execution chain. It is
/// not invoked directly — call [`PrintTarget::with_writer`] to build a
/// `PrintWriter` on demand (used by each VM transition) and [`drain_py`] or
/// [`snapshot_py`] to read the collected buffer back out.
///
/// # Foot-guns
///
/// - `Collect` holds an `Arc`; cloning is cheap but **shares** the buffer. Use
///   [`PrintTarget::clone_handle`] instead of `Clone` so the intent is explicit.
/// - Draining (`drain_py`) empties the buffer. After an error path drains,
///   subsequent snapshot access would return an empty list. This is by design:
///   the error consumes the buffer.
#[derive(Debug, Default)]
pub(crate) enum PrintTarget {
    /// Print goes to process stdout — the default when no `print_callback` is set.
    #[default]
    Stdout,
    /// Each fragment is forwarded to a Python callable as `(stream_name, text)`.
    Callback(Py<PyAny>),
    /// Each fragment accumulates into a shared buffer readable via `drain_py` /
    /// `snapshot_py` and exposed on `MontyComplete.print_output` etc.
    Collect(CollectBuffer),
}

impl PrintTarget {
    /// Parses a Python `print_callback` argument into a `PrintTarget`.
    ///
    /// Accepts `None`, the string `'collect'`, or a callable. Any other value
    /// is a `TypeError` so mistakes surface eagerly rather than during execution.
    pub fn from_py(value: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let Some(obj) = value else {
            return Ok(Self::Stdout);
        };
        if let Ok(s) = obj.cast::<PyString>() {
            let s = s.to_cow()?;
            if s == "collect" {
                Ok(Self::Collect(Arc::default()))
            } else {
                Err(PyTypeError::new_err(format!(
                    "print_callback string must be 'collect', got {:?}",
                    s.as_ref()
                )))
            }
        } else if obj.is_callable() {
            Ok(Self::Callback(obj.clone().unbind()))
        } else {
            Err(PyTypeError::new_err(
                "print_callback must be a callable, 'collect', or None",
            ))
        }
    }

    /// Returns a fresh `PrintTarget` that targets the same sink as `self`.
    ///
    /// - `Stdout` → `Stdout` (nothing to share).
    /// - `Callback` → clones the `Py<PyAny>` reference (another handle to the
    ///   same callable).
    /// - `Collect` → clones the `Arc`, so the new target **writes into the same
    ///   buffer**. This is the desired behavior for threading the target
    ///   through `start`/`resume` chains and into `spawn_blocking` workers.
    pub fn clone_handle(&self, py: Python<'_>) -> Self {
        match self {
            Self::Stdout => Self::Stdout,
            Self::Callback(cb) => Self::Callback(cb.clone_ref(py)),
            Self::Collect(arc) => Self::Collect(arc.clone()),
        }
    }

    /// Builds a `PrintWriter` for a single VM transition and invokes `f` with it.
    ///
    /// The writer borrows from this target for the duration of `f`, so the
    /// closure shape keeps lifetimes sound. For `Collect`, the internal mutex
    /// is held for the entirety of `f` — that is fine because a single VM
    /// transition is synchronous and the only other user of the buffer is
    /// `snapshot_py` which only runs between transitions.
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
            Self::Callback(cb) => PrintStorage::Callback(CallbackStringPrint::from_ref(cb)),
            Self::Collect(arc) => PrintStorage::Collect(arc.lock().unwrap_or_else(PoisonError::into_inner)),
        }
    }

    /// Takes the collected buffer out of the target and converts it to a
    /// Python `list[tuple[str, str]]`.
    ///
    /// Intended for terminal paths — `MontyComplete` on success, or the
    /// `print_output` attribute of `MontyRuntimeError` on failure. Returns
    /// `None` for non-`Collect` targets so the Python attribute is `None`
    /// (rather than an empty list) in those modes.
    pub fn drain_py(&self, py: Python<'_>) -> Option<Py<PyList>> {
        let Self::Collect(arc) = self else {
            return None;
        };
        let mut guard = arc.lock().unwrap_or_else(PoisonError::into_inner);
        let items = mem::take(&mut *guard);
        Some(vec_to_py_list(py, items).expect("failed to build print_output list"))
    }

    /// Non-draining peek for snapshot `print_output` getters.
    ///
    /// Clones the current buffer contents into a Python list, leaving the
    /// underlying Vec untouched so further `resume()` calls continue to
    /// accumulate. Returns `None` for non-`Collect` targets.
    pub fn snapshot_py(&self, py: Python<'_>) -> Option<Py<PyList>> {
        let Self::Collect(arc) = self else {
            return None;
        };
        let guard = arc.lock().unwrap_or_else(PoisonError::into_inner);
        Some(vec_to_py_list(py, guard.clone()).expect("failed to build print_output list"))
    }
}

/// Builds the Python list that is exposed as `print_output`.
///
/// Each entry is a 2-tuple `(stream_label, text)`. The stream labels are
/// interned module-level strings so all entries share one `PyString` per
/// stream, keeping memory down for large collected outputs.
fn vec_to_py_list(py: Python<'_>, items: Vec<(PrintStream, String)>) -> PyResult<Py<PyList>> {
    let list = PyList::empty(py);
    for (stream, text) in items {
        let label = match stream {
            PrintStream::Stdout => intern!(py, "stdout"),
            PrintStream::Stderr => intern!(py, "stderr"),
        };
        let tuple = PyTuple::new(py, [label.clone().into_any(), PyString::new(py, &text).into_any()])?;
        list.append(tuple)?;
    }
    Ok(list.unbind())
}

/// Live writer storage — owns the per-call backing (mutex guard, callback
/// wrapper) that a `PrintWriter` points into.
///
/// Produced by [`PrintTarget::storage`] and consumed by repeatedly calling
/// [`PrintStorage::writer`] (which hands out a fresh `PrintWriter` each time
/// with lifetime tied to this storage). This two-step split exists because
/// `PrintWriter::Collect` variants need `&mut` access to a locked buffer, and
/// `PrintWriter::Callback` needs `&mut` access to a `CallbackStringPrint`
/// value — both of which must outlive the writer.
pub(crate) enum PrintStorage<'a> {
    /// No-op storage — the writer just targets stdout.
    Stdout,
    /// Owned callback wrapper (holds a `Py<PyAny>` handle).
    Callback(CallbackStringPrint),
    /// Live `MutexGuard` over the shared collect buffer, held for as long as
    /// this storage exists.
    Collect(MutexGuard<'a, Vec<(PrintStream, String)>>),
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
            Self::Collect(guard) => PrintWriter::CollectTuples(guard),
        }
    }
}

/// `PrintWriterCallback` adaptor that forwards each fragment to a Python callable.
///
/// Holds a GIL-independent `Py<PyAny>` reference so it can be used across GIL
/// release boundaries; the GIL is re-acquired briefly for each invocation.
#[derive(Debug)]
pub(crate) struct CallbackStringPrint(Py<PyAny>);

impl CallbackStringPrint {
    /// Creates a wrapper that shares a fresh reference to the given callable.
    fn from_ref(callback: &Py<PyAny>) -> Self {
        Self(Python::attach(|py| callback.clone_ref(py)))
    }
}

impl PrintWriterCallback for CallbackStringPrint {
    fn stdout_write(&mut self, output: Cow<'_, str>) -> Result<(), MontyException> {
        Python::attach(|py| {
            self.0.bind(py).call1(("stdout", output.as_ref()))?;
            Ok::<_, PyErr>(())
        })
        .map_err(|e| Python::attach(|py| exc_py_to_monty(py, &e)))
    }

    fn stdout_push(&mut self, end: char) -> Result<(), MontyException> {
        Python::attach(|py| {
            self.0.bind(py).call1(("stdout", end.to_string()))?;
            Ok::<_, PyErr>(())
        })
        .map_err(|e| Python::attach(|py| exc_py_to_monty(py, &e)))
    }
}
