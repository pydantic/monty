//! Python ↔ Monty value conversion (the `python` cargo feature).
//!
//! Bidirectional conversions between PyO3 Python objects and the boundary's
//! value arenas (`MontyGraph`) and `MontyException`, shared by every embedder
//! that hosts a real CPython (currently the `pydantic-monty` extension module).
//! Sharing is preserved both ways: one host object referenced twice in a
//! message crosses as one node, and one node decodes to one Python object.
//! Lives here (rather than in `pydantic-monty`) so consumers depend on one
//! leaf crate instead of linking the whole extension module as an rlib.
//!
//! pyo3's `extension-module` feature is deliberately NOT enabled by this crate:
//! the top-level crate decides how libpython is linked (e.g. maturin enables
//! it for wheels).

mod class_instance;
mod convert;
mod decode;
mod encode;
mod exceptions;

pub use class_instance::{InstanceStore, PyMontyClassProxy, PyMontyClassTypeProxy, uuid_to_py};
pub use convert::PyMontyFileHandle;
pub use decode::{DecodedArena, monty_to_py};
pub use encode::{GraphEncoder, py_to_monty, py_to_monty_value};
pub use exceptions::{exc_monty_to_py, exc_py_to_monty, exc_to_monty_node};
