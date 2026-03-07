//! Shared helpers for snapshot-extension integration tests.

use monty::{
    ExtFunctionResult, MontyObject, MontyRepl, MontyRun, NoLimitTracker, PrintWriter, ReplProgress, RunProgress,
    SnapshotExtension,
};

/// Extension methods on `RunProgress` and `ReplProgress` for snapshot tests.
pub trait ProgressSnapshotExt: Sized {
    /// Attaches snapshot-extension bytes to the progress value when it carries a snapshot.
    fn attach_snapshot_extension(self, ext: Vec<u8>) -> Self;
    /// Reads snapshot-extension bytes from the progress value when available.
    fn get_snapshot_extension(&self) -> Option<&[u8]>;
    /// Drives the progress to `ResolveFutures`, panicking on unexpected variants.
    fn drive_to_resolve_futures(self) -> Self;
    /// Completes a `ResolveFutures` progress with a repeated return value.
    fn complete_resolve_futures(self, return_value: &MontyObject) -> Self;
}

/// Generates `ProgressSnapshotExt` impls for progress enums that differ only in
/// the shape of their complete variant.
macro_rules! impl_progress_ext {
    ($Progress:ident, $complete_pat:pat => $complete_expr:expr, $complete_get_pat:pat, $complete_panic_pat:pat) => {
        impl ProgressSnapshotExt for $Progress<NoLimitTracker> {
            fn attach_snapshot_extension(self, snapshot_extension: Vec<u8>) -> Self {
                match self {
                    Self::FunctionCall(call) => Self::FunctionCall(call.with_snapshot_extension(snapshot_extension)),
                    Self::OsCall(call) => Self::OsCall(call.with_snapshot_extension(snapshot_extension)),
                    Self::ResolveFutures(state) => {
                        Self::ResolveFutures(state.with_snapshot_extension(snapshot_extension))
                    }
                    Self::NameLookup(lookup) => Self::NameLookup(lookup.with_snapshot_extension(snapshot_extension)),
                    $complete_pat => $complete_expr,
                }
            }

            fn get_snapshot_extension(&self) -> Option<&[u8]> {
                match self {
                    Self::FunctionCall(call) => call.snapshot_extension().map(SnapshotExtension::as_slice),
                    Self::OsCall(call) => call.snapshot_extension().map(SnapshotExtension::as_slice),
                    Self::ResolveFutures(state) => state.snapshot_extension().map(SnapshotExtension::as_slice),
                    Self::NameLookup(lookup) => lookup.snapshot_extension().map(SnapshotExtension::as_slice),
                    $complete_get_pat => None,
                }
            }

            fn drive_to_resolve_futures(mut self) -> Self {
                loop {
                    match self {
                        Self::FunctionCall(call) => {
                            self = call
                                .resume_pending(&mut PrintWriter::Stdout)
                                .expect("run_pending should succeed");
                        }
                        resolved @ Self::ResolveFutures(_) => return resolved,
                        Self::OsCall(call) => panic!("unexpected OsCall: {:?}", call.function),
                        Self::NameLookup(lookup) => panic!("unexpected NameLookup: {}", lookup.name),
                        $complete_panic_pat => panic!("unexpected Complete before ResolveFutures"),
                    }
                }
            }

            fn complete_resolve_futures(self, return_value: &MontyObject) -> Self {
                let Self::ResolveFutures(state) = self else {
                    panic!("expected resolve futures progress");
                };
                let results = state
                    .pending_call_ids()
                    .iter()
                    .map(|call_id| (*call_id, ExtFunctionResult::Return(return_value.clone())))
                    .collect();
                state
                    .resume(results, &mut PrintWriter::Stdout)
                    .expect("resume should succeed")
            }
        }
    };
}

impl_progress_ext!(
    RunProgress,
    Self::Complete(value) => Self::Complete(value),
    Self::Complete(_),
    Self::Complete(_)
);
impl_progress_ext!(
    ReplProgress,
    Self::Complete { repl, value } => Self::Complete { repl, value },
    Self::Complete { .. },
    Self::Complete { .. }
);

/// Progress variants relevant to snapshot-extension round-trip coverage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotProgressVariant {
    /// Suspension on a host function call.
    FunctionCall,
    /// Suspension on a host OS interaction.
    OsCall,
    /// Suspension while awaiting unresolved external futures.
    ResolveFutures,
    /// Completed execution with no suspension snapshot to decorate.
    Complete,
}

/// Expected snapshot-extension visibility for a progress variant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotBehavior {
    /// The snapshot extension should be present and match the attached bytes.
    Preserved,
    /// The progress value has no suspendable snapshot, so no extension is visible.
    Absent,
}

/// Triggers an external function call suspension.
const EXTERNAL_CALL_SCRIPT: &str = "ext_fn([])";
/// Triggers an OS-level call suspension via filesystem access.
const OS_CALL_SCRIPT: &str = "from pathlib import Path; Path('/tmp/test.txt').exists()";
/// Completes synchronously without suspension.
const COMPLETE_SCRIPT: &str = "1 + 2";
/// Suspends on `ResolveFutures` after an initial function call.
const RESOLVE_FUTURES_SCRIPT: &str = r"
import asyncio

async def main():
    return await foo()

await main()
";

/// Describes how a test variant should be driven from source text.
enum ScriptAction {
    /// Start the script and use the initial progress directly.
    Plain(&'static str),
    /// Start the script and drive the initial progress to `ResolveFutures`.
    ResolveThen(&'static str),
    /// Start the script and expect it to complete without suspension.
    RunComplete(&'static str),
}

/// Maps a progress variant to the script and follow-up action needed to build it.
fn script_for_variant(variant: SnapshotProgressVariant) -> ScriptAction {
    match variant {
        SnapshotProgressVariant::FunctionCall => ScriptAction::Plain(EXTERNAL_CALL_SCRIPT),
        SnapshotProgressVariant::OsCall => ScriptAction::Plain(OS_CALL_SCRIPT),
        SnapshotProgressVariant::ResolveFutures => ScriptAction::ResolveThen(RESOLVE_FUTURES_SCRIPT),
        SnapshotProgressVariant::Complete => ScriptAction::RunComplete(COMPLETE_SCRIPT),
    }
}

/// Creates a `RunProgress` from Python source, which may suspend or complete
/// immediately for non-suspending scripts.
pub fn create_run_progress(script: &str) -> RunProgress<NoLimitTracker> {
    let runner = MontyRun::new(script.to_owned(), "test.py", vec![]).expect("runner creation should succeed");
    runner
        .start(vec![], NoLimitTracker, &mut PrintWriter::Stdout)
        .expect("run should produce progress")
}

/// Creates a reusable REPL instance for snapshot-extension tests.
pub fn create_repl() -> MontyRepl<NoLimitTracker> {
    let (repl, _result) = MontyRepl::new(
        "pass".to_owned(),
        "init.py",
        vec![],
        vec![],
        NoLimitTracker,
        &mut PrintWriter::Stdout,
    )
    .expect("repl creation should succeed");
    repl
}

/// Creates a `RunProgress` for the requested variant.
pub fn create_run_progress_for_variant(variant: SnapshotProgressVariant) -> RunProgress<NoLimitTracker> {
    match script_for_variant(variant) {
        ScriptAction::Plain(script) => create_run_progress(script),
        ScriptAction::ResolveThen(script) => create_run_progress(script).drive_to_resolve_futures(),
        ScriptAction::RunComplete(script) => {
            let runner = MontyRun::new(script.to_owned(), "test.py", vec![]).expect("runner creation should succeed");
            let progress = runner
                .start(vec![], NoLimitTracker, &mut PrintWriter::Stdout)
                .expect("run should complete");
            assert!(
                matches!(&progress, RunProgress::Complete(_)),
                "expected RunComplete script to produce RunProgress::Complete"
            );
            progress
        }
    }
}

/// Creates a `ReplProgress` for the requested variant.
pub fn create_repl_progress_for_variant(variant: SnapshotProgressVariant) -> ReplProgress<NoLimitTracker> {
    let repl = create_repl();
    match script_for_variant(variant) {
        ScriptAction::Plain(script) => repl
            .start(script, &mut PrintWriter::Stdout)
            .expect("repl should produce progress"),
        ScriptAction::RunComplete(script) => {
            let progress = repl
                .start(script, &mut PrintWriter::Stdout)
                .expect("repl should produce progress");
            assert!(
                matches!(&progress, ReplProgress::Complete { .. }),
                "expected RunComplete script to produce ReplProgress::Complete"
            );
            progress
        }
        ScriptAction::ResolveThen(script) => repl
            .start(script, &mut PrintWriter::Stdout)
            .expect("repl should produce progress")
            .drive_to_resolve_futures(),
    }
}
