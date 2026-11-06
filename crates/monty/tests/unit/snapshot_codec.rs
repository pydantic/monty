//! Private compatibility regression for the persisted snapshot fields.

use super::*;
use crate::run::{CompileOptions, MontyRun};

/// The ownership guard keeps the flat future fields written by earlier v15 builds readable.
#[test]
fn legacy_flat_future_snapshot_resumes() {
    /// Named fields used before suspended roots gained an ownership guard.
    #[derive(serde::Serialize)]
    struct LegacyFuture<'a> {
        executor: &'a Executor,
        vm_state: &'a VMSnapshot,
        heap: &'a Heap,
        pending_call_ids: &'a [u32],
        position: &'a SourceRange,
    }

    let runner = MontyRun::new(
        "async def helper():\n    return await external()\nawait helper()".to_owned(),
        "legacy.py",
        vec!["external".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();
    let call = runner
        .start(
            vec![MontyObject::function("external".to_owned(), None)],
            ResourceTracker::default(),
            PrintWriter::Disabled,
        )
        .unwrap()
        .into_function_call()
        .unwrap();
    let state = call
        .resume_pending(PrintWriter::Disabled)
        .unwrap()
        .into_resolve_futures()
        .unwrap();
    let legacy = LegacyFuture {
        executor: &state.snapshot.executor,
        vm_state: &state.snapshot.vm_state,
        heap: &state.snapshot.heap,
        pending_call_ids: &state.pending_call_ids,
        position: &state.position,
    };
    let mut bytes = Vec::new();
    serde::Serialize::serialize(&legacy, &mut minicbor_serde::Serializer::new(&mut bytes)).unwrap();
    let loaded: ResolveFutures =
        serde::Deserialize::deserialize(&mut minicbor_serde::Deserializer::new(&bytes)).unwrap();
    let call_id = loaded.pending_call_ids()[0];
    let done = loaded
        .resume(
            vec![(call_id, ExtFunctionResult::Return(MontyObject::int(42)))],
            PrintWriter::Disabled,
        )
        .unwrap();
    assert_eq!(done.into_complete(), Some(MontyObject::int(42)));
}
