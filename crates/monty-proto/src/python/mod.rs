//! Python ↔ Monty value conversion (the `python` cargo feature).
//!
//! Bidirectional conversions between PyO3 Python objects and the boundary's
//! value arenas (`MontyGraph`) and `MontyException`, shared by every embedder
//! that hosts a real CPython: the `pydantic-monty` extension module, and the
//! embedded-CPython worker `monty-cpython` (in the private `monty-private` repository).
//! Sharing is preserved both ways for containers and wrappers: one such host
//! object referenced twice in a message crosses as one node, and one node
//! decodes to one Python object (leaves are re-encoded per reference).
//! Lives here (rather than in `pydantic-monty`) so consumers depend on one
//! leaf crate instead of linking the whole extension module as an rlib.
//!
//! It must not import from `pydantic_monty`, which depends on this crate: a Python class it converts
//! to or checks for, such as `MontyNamedTuple` (`named_tuple.rs`), is defined here and re-exported
//! by `pydantic_monty`. Other imports come only from the Python standard library.
//! The exception is `class_instance.rs`, which still imports the `ClassInstance` and `ClassType`
//! wrappers from `pydantic_monty.class_instance` to recognise them.
//!
//! pyo3's `extension-module` feature is deliberately NOT enabled by this crate:
//! the top-level crate decides how libpython is linked (e.g. maturin enables
//! it for wheels).

mod class_instance;
mod convert;
mod decode;
mod encode;
mod exceptions;
mod external_module;
mod named_tuple;
mod std_type_proxy;

pub use class_instance::{
    InstanceStore, PyMontyClassProxy, PyMontyClassTypeProxy, is_class_instance_wrapper, is_class_type_wrapper,
    uuid_to_py,
};
pub use convert::{CoroutineFunctionExt, PyMontyFileHandle, get_docstring, py_type_object_to_monty};
pub use decode::{DecodedArena, monty_to_py};
pub use encode::{GraphEncoder, py_to_monty, py_to_monty_value};
pub use exceptions::{exc_monty_to_py, exc_py_to_monty, exc_to_monty_node};
pub use external_module::PyExternalModule;
pub use named_tuple::monty_named_tuple_type;
pub use std_type_proxy::PyMontyStdTypeProxy;
