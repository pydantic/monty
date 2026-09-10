use std::sync::Arc;

use monty_proto::python::InstanceStore;
use pyo3::{exceptions::PyMemoryError, prelude::*};
use tokio::sync::Mutex;

use super::{DriveContext, SnapshotState};
use crate::print_target::PrintTarget;

fn snapshot(py: Python<'_>) -> SnapshotState {
    let callback = py.eval(c"lambda stream, text: None", None, None).unwrap();
    SnapshotState::new(DriveContext::new(
        Arc::new(Mutex::new(None)),
        InstanceStore::new(py),
        PrintTarget::from_py(Some(&callback)).unwrap(),
        "test.py".to_owned(),
        None,
        None,
    ))
}

#[test]
fn failed_claim_preparation_is_retryable() {
    Python::initialize();
    Python::attach(|py| {
        let snapshot = snapshot(py);
        let error = snapshot
            .claim_with::<()>(py, |_| Err(PyMemoryError::new_err("context allocation failed")))
            .err()
            .expect("preparation must fail");
        assert!(error.is_instance_of::<PyMemoryError>(py));
        assert!(snapshot.claim(py).is_ok());
        assert!(snapshot.claim(py).is_err());
    });
}

#[test]
fn reentrant_claim_wins_over_pending_preparation() {
    Python::initialize();
    Python::attach(|py| {
        let snapshot = snapshot(py);
        let error = snapshot
            .claim_with(py, |_| snapshot.claim(py).map(|_| ()))
            .err()
            .expect("the inner claim consumed the snapshot");
        assert_eq!(error.to_string(), "RuntimeError: snapshot has already been resumed");
        assert!(snapshot.claim(py).is_err());
    });
}
