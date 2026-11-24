//! Named tuples decode and encode without importing the `pydantic_monty` package.
//!
//! `monty-cpython` shares this conversion code in an interpreter that does not have the package, so
//! the class must come from `monty-proto` itself.

use monty_proto::python::{InstanceStore, monty_to_py, py_to_monty};
use monty_types::MontyObject;
use pyo3::prelude::*;

/// A decoded named tuple works, round-trips, and leaves `pydantic_monty` unimported.
#[test]
fn named_tuples_convert_without_importing_the_package() {
    Python::initialize();
    Python::attach(|py| {
        let store = InstanceStore::new(py);
        let point = MontyObject::named_tuple("Point", ["x", "y"], [MontyObject::int(1), MontyObject::int(2)]);
        let decoded = monty_to_py(py, &point, &store).unwrap().into_bound(py);

        assert_eq!(decoded.repr().unwrap().to_str().unwrap(), "Point(x=1, y=2)");
        assert_eq!(decoded.getattr("y").unwrap().extract::<i64>().unwrap(), 2);
        assert_eq!(py_to_monty(&decoded, &store).unwrap(), point);

        let modules = py.import("sys").unwrap().getattr("modules").unwrap();
        assert!(!modules.contains("pydantic_monty").unwrap());
    });
}
