//! Asyncio can drive Monty from inside an embedding Tokio runtime.

use std::{future::ready, time::Duration};

use pyo3::{exceptions::PyRuntimeError, prelude::*};
use tokio::{
    runtime::{Builder, RuntimeFlavor},
    task::consume_budget,
    time::sleep,
};

use crate::{async_driver::future_into_py, pool::block_on_sync};

/// Uses Monty's timers without nesting `block_on` or depending on the embedder's budget.
#[test]
fn asyncio_runs_inside_tokio() {
    Python::initialize();
    for runtime in [
        Builder::new_current_thread().build().unwrap(),
        Builder::new_multi_thread().worker_threads(1).build().unwrap(),
    ] {
        let multithread = runtime.handle().runtime_flavor() == RuntimeFlavor::MultiThread;
        runtime.block_on(async {
            Python::attach(|py| {
                let operation = future_into_py(py, async move {
                    for _ in 0..256 {
                        consume_budget().await;
                    }
                    sleep(Duration::from_millis(1)).await;
                    // Sync re-entry must see the original scheduler, not Monty's multi-thread handle.
                    let sync_result = block_on_sync(ready(42));
                    if multithread {
                        assert_eq!(sync_result.unwrap(), 42);
                    } else {
                        Python::attach(|py| assert!(sync_result.unwrap_err().is_instance_of::<PyRuntimeError>(py)));
                    }
                    Ok(42)
                })
                .unwrap();
                let asyncio = py.import("asyncio").unwrap();
                let bounded = asyncio.call_method1("wait_for", (operation, 5)).unwrap();
                let result = asyncio.call_method1("run", (bounded,)).unwrap();
                assert_eq!(result.extract::<i32>().unwrap(), 42);
            });
        });
    }
}
