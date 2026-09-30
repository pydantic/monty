use monty_types::{ExtFunctionResult, MontyObject};
use pyo3::Python;
use tokio::{
    runtime::Builder,
    task::{JoinSet, yield_now},
};

use super::wait_for_futures;

#[test]
fn completed_batches_discard_manually_resolved_results() {
    Python::initialize();
    Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            for pending_id in [1, 2] {
                let mut tasks = JoinSet::new();
                let handles: Vec<_> = (0..2)
                    .map(|id| tasks.spawn(async move { (id, ExtFunctionResult::Return(MontyObject::none())) }))
                    .collect();
                // Complete both tasks before polling to exercise the batched path.
                while handles.iter().any(|handle| !handle.is_finished()) {
                    yield_now().await;
                }
                let result = wait_for_futures(&mut tasks, &[pending_id]).await;
                if pending_id == 1 {
                    let ids: Vec<_> = result.unwrap().into_iter().map(|(id, _)| id).collect();
                    assert_eq!(ids, vec![1]);
                } else {
                    assert_eq!(
                        result.unwrap_err().to_string(),
                        "RuntimeError: No pending async tasks but ResolveFutures requested"
                    );
                }
            }
        });
}
