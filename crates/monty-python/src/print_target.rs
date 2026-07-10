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
//! [`PrintTarget`] value through `feed_run`, while the collector objects
//! themselves remain the single public place that exposes the captured output.

use std::sync::{Arc, Mutex, PoisonError};

use monty::{MontyException, PrintStream};
use monty_proto::python::exc_py_to_monty;
use pyo3::{
    PyRef,
    exceptions::PyTypeError,
    intern,
    prelude::*,
    types::{PyList, PyString},
};

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

    /// Returns a fresh `PrintTarget` targeting the same sink as `self`. The
    /// collector variants clone their `Arc`, so the new target **writes into the
    /// same buffer** — exactly what threading through `start`/`resume` chains and
    /// `spawn_blocking` workers needs.
    ///
    /// Used instead of `Clone` to make the share-vs-copy intent explicit.
    /// Callers without a `Python` token should use
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

    /// Delivers one already-formatted output fragment to this target.
    ///
    /// Used by pool sessions, where `print()` output arrives from the
    /// worker process as pre-rendered `(stream, text)` events rather than
    /// through a `PrintWriter`. Safe to call without the GIL held — the
    /// `Callback` variant attaches internally.
    pub fn write_event(&self, stream: PrintStream, text: &str) -> Result<(), MontyException> {
        match self {
            Self::Stdout => {
                match stream {
                    PrintStream::Stdout => print!("{text}"),
                    PrintStream::Stderr => eprint!("{text}"),
                }
                Ok(())
            }
            Self::Callback(cb) => Python::attach(|py| {
                let stream_name = match stream {
                    PrintStream::Stdout => "stdout",
                    PrintStream::Stderr => "stderr",
                };
                cb.bind(py).call1((stream_name, text))?;
                Ok::<_, PyErr>(())
            })
            .map_err(|e| Python::attach(|py| exc_py_to_monty(py, &e))),
            Self::CollectStreams(buf) => {
                buf.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((stream, text.to_owned()));
                Ok(())
            }
            Self::CollectString(buf) => {
                buf.lock().unwrap_or_else(PoisonError::into_inner).push_str(text);
                Ok(())
            }
        }
    }
}
