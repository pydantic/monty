//! Python ↔ Monty value conversion (the `python` cargo feature).
//!
//! Bidirectional conversions between PyO3 Python objects and Monty's
//! `MontyObject`/`MontyException` carrier types, shared by every embedder that
//! hosts a real CPython (currently the `pydantic-monty` extension module).
//! Lives here (rather than in `pydantic-monty`) so consumers depend on one
//! leaf crate instead of linking the whole extension module as an rlib.
//!
//! pyo3's `extension-module` feature is deliberately NOT enabled by this crate:
//! the top-level crate decides how libpython is linked (e.g. maturin enables
//! it for wheels).

mod convert;
mod dataclass;
mod exceptions;

pub use convert::{PyMontyFileHandle, monty_to_py, py_to_monty, py_to_monty_value};
pub use dataclass::{DcRegistry, PyUnknownDataclass};
pub use exceptions::{exc_monty_to_py, exc_py_to_monty, exc_to_monty_object};
