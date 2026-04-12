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

use crate::exceptions::{MontyError, exc_py_to_monty};

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
    /// - `Callback` → clones the `Py<PyAny>` reference using the provided GIL
    ///   token.
    /// - `Collect` → clones the `Arc`, so the new target **writes into the same
    ///   buffer**. This is the desired behavior for threading the target
    ///   through `start`/`resume` chains and into `spawn_blocking` workers.
    ///
    /// Used instead of `Clone` to make the share-vs-copy intent explicit.
    /// Callers without a `Python` token in scope should use
    /// [`clone_handle_detached`](Self::clone_handle_detached) instead.
    pub fn clone_handle(&self, py: Python<'_>) -> Self {
        match self {
            Self::Stdout => Self::Stdout,
            Self::Callback(cb) => Self::Callback(cb.clone_ref(py)),
            Self::Collect(arc) => Self::Collect(arc.clone()),
        }
    }

    /// Detached variant of [`clone_handle`](Self::clone_handle) for callers
    /// running without the GIL held (e.g. inside an `async move` block or a
    /// `spawn_blocking` worker about to hand the clone to another thread).
    ///
    /// Acquires the GIL internally only when the `Callback` variant actually
    /// needs it; `Stdout` and `Collect` skip the acquisition entirely.
    pub fn clone_handle_detached(&self) -> Self {
        match self {
            Self::Stdout => Self::Stdout,
            Self::Callback(_) => Python::attach(|py| self.clone_handle(py)),
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
            // Borrow the callback rather than clone it — the storage's lifetime
            // is bounded by the target, so there is no need to bump the Py ref
            // count per VM transition (which would require reacquiring the GIL
            // inside `py.detach`).
            Self::Callback(cb) => PrintStorage::Callback(CallbackStringPrint(cb)),
            Self::Collect(arc) => PrintStorage::Collect(arc.lock().unwrap_or_else(PoisonError::into_inner)),
        }
    }

    /// Takes the collected buffer out of the target and converts it to a
    /// Python `list[tuple[str, str]]`.
    ///
    /// Intended for terminal paths — `MontyComplete` on success, or the
    /// `print_output` attribute of `MontyRuntimeError` on failure. Returns
    /// `Ok(None)` for non-`Collect` targets so the Python attribute is `None`
    /// (rather than an empty list) in those modes.
    ///
    /// Returns `Err` if building the Python list fails (rare — interpreter
    /// shutdown or allocation failure). Callers should propagate with `?`.
    pub fn drain_py(&self, py: Python<'_>) -> PyResult<Option<Py<PyList>>> {
        let Self::Collect(arc) = self else {
            return Ok(None);
        };
        let mut guard = arc.lock().unwrap_or_else(PoisonError::into_inner);
        let items = mem::take(&mut *guard);
        Ok(Some(vec_to_py_list(py, items)?))
    }

    /// Drains the collect buffer and builds a `MontyError` for `exc`, attaching
    /// the drained `print_output` list.
    ///
    /// If building the Python list fails (rare — interpreter shutdown or
    /// allocation failure), returns that `PyErr` instead; the underlying
    /// Monty exception is dropped in that case, but the drain failure is
    /// what the user actually needs to see.
    pub fn drain_into_err(&self, py: Python<'_>, exc: MontyException) -> PyErr {
        match self.drain_py(py) {
            Ok(list) => MontyError::new_err(py, exc, list),
            Err(err) => err,
        }
    }

    /// Non-draining peek for snapshot `print_output` getters.
    ///
    /// Clones the current buffer contents into a Python list, leaving the
    /// underlying Vec untouched so further `resume()` calls continue to
    /// accumulate. Returns `Ok(None)` for non-`Collect` targets.
    pub fn snapshot_py(&self, py: Python<'_>) -> PyResult<Option<Py<PyList>>> {
        let Self::Collect(arc) = self else {
            return Ok(None);
        };
        let guard = arc.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(Some(vec_to_py_list(py, guard.clone())?))
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
    /// Borrowed callback wrapper — points at the `Py<PyAny>` owned by the
    /// parent `PrintTarget::Callback` variant.
    Callback(CallbackStringPrint<'a>),
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
