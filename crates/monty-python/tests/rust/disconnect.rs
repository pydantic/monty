//! Python exception conversion when the Rust caller has no transport evidence.

use monty_pool::PoolError;
use pyo3::prelude::*;

use crate::{MontyDisconnectError, pool::pool_err_to_py};

/// Missing transport evidence must remain None rather than inventing a Close code or reason.
#[test]
fn disconnect_without_transport_evidence_preserves_unknown_fields() {
    Python::initialize();
    Python::attach(|py| {
        let error = pool_err_to_py(
            py,
            PoolError::Disconnected {
                context: "waiting for a reply".to_owned(),
                websocket: None,
            },
        );
        assert!(error.is_instance_of::<MontyDisconnectError>(py));
        let value = error.value(py);
        assert_eq!(
            value.str().unwrap().to_str().unwrap(),
            "monty worker connection closed while waiting for a reply"
        );
        for field in ["stage", "close_code", "close_reason", "transport_error"] {
            assert!(value.getattr(field).unwrap().is_none(), "{field}");
        }
    });
}
