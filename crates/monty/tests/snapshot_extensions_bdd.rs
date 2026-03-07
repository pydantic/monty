//! Behavioural coverage for snapshot extension byte persistence.

use monty::{MontyRun, NoLimitTracker, PrintWriter, ReplProgress, RunProgress};
use rstest::fixture;
use rstest_bdd_macros::{given, scenario, then, when};
use snapshot_test_utils::{ProgressSnapshotExt, create_repl};

#[expect(
    dead_code,
    reason = "BDD scenarios share a helper module with non-BDD snapshot tests"
)]
#[path = "support/snapshot_test_utils.rs"]
mod snapshot_test_utils;

/// Shared world state for snapshot-extension BDD scenarios.
///
/// `script` stores the Python source for run-based tests. `repl_snippet`
/// stores the REPL input under test. `snapshot_extension` holds the raw binary
/// metadata to attach before serialization. `loaded_snapshot_extension` is
/// populated with the bytes recovered after a successful load. `load_failed`
/// records scenarios where deserialization is expected to fail.
#[derive(Default)]
struct SnapshotExtensionsWorld {
    script: String,
    repl_snippet: String,
    snapshot_extension: Vec<u8>,
    loaded_snapshot_extension: Option<Vec<u8>>,
    load_failed: bool,
}

/// Round-trip operations shared by run and REPL progress values in BDD steps.
trait BddRoundTripProgress: ProgressSnapshotExt + Sized {
    /// Serializes the current progress value for round-trip checks.
    fn dump_progress(&self) -> Vec<u8>;
    /// Reports whether loading the serialized bytes fails.
    fn load_fails(bytes: &[u8]) -> bool;
}

/// Implements BDD round-trip helpers for a concrete progress type.
macro_rules! impl_bdd_round_trip_progress {
    ($Progress:ident) => {
        impl BddRoundTripProgress for $Progress<NoLimitTracker> {
            fn dump_progress(&self) -> Vec<u8> {
                self.dump().expect("progress dump should succeed")
            }

            fn load_fails(bytes: &[u8]) -> bool {
                Self::load(bytes).is_err()
            }
        }
    };
}

impl_bdd_round_trip_progress!(RunProgress);
impl_bdd_round_trip_progress!(ReplProgress);

/// Corrupts a serialized progress payload and reports whether reload fails.
fn corrupted_progress_fails_to_load<P: BddRoundTripProgress>(progress: &P) -> bool {
    let mut bytes = progress.dump_progress();
    bytes.pop();
    P::load_fails(&bytes)
}

/// Creates the per-scenario world that records script inputs and load results.
#[fixture]
fn world() -> SnapshotExtensionsWorld {
    SnapshotExtensionsWorld::default()
}

/// Sets the run script to a suspendable program with a single external call.
#[given("a suspendable script with one external call")]
fn given_suspendable_script(world: &mut SnapshotExtensionsWorld) {
    world.script = String::from("ext_fn([])");
}

/// Sets the REPL snippet to a suspendable expression with one external call.
#[given("a REPL snippet with one external call")]
fn given_repl_snippet(world: &mut SnapshotExtensionsWorld) {
    world.repl_snippet = String::from("ext_fn([])");
}

/// Provides the raw snapshot-extension bytes attached before serialization.
#[given("snapshot extension bytes")]
fn given_snapshot_extension_bytes(world: &mut SnapshotExtensionsWorld) {
    world.snapshot_extension = vec![1, 3, 5, 7];
}

/// Attaches extension bytes, performs a dump/load round-trip, and returns
/// the recovered extension bytes.
macro_rules! dump_load_round_trip {
    ($progress:expr, $ext:expr, $ProgressType:ident) => {{
        let progress = $progress.attach_snapshot_extension($ext);
        let bytes = progress
            .dump()
            .expect(concat!(stringify!($ProgressType), " dump should succeed"));
        let loaded: $ProgressType<NoLimitTracker> =
            $ProgressType::load(&bytes).expect(concat!(stringify!($ProgressType), " load should succeed"));
        loaded.get_snapshot_extension().map(<[u8]>::to_vec)
    }};
}

/// Starts run progress, attaches the test extension bytes, and records the
/// bytes recovered after a dump/load round trip.
#[when("run progress is dumped and loaded with snapshot extension bytes")]
fn when_run_progress_dumped_and_loaded(world: &mut SnapshotExtensionsWorld) {
    let runner = MontyRun::new(world.script.clone(), "test.py", vec![]).expect("runner creation should succeed");
    let progress = runner
        .start(vec![], NoLimitTracker, &mut PrintWriter::Stdout)
        .expect("run should suspend");
    world.loaded_snapshot_extension = dump_load_round_trip!(progress, world.snapshot_extension.clone(), RunProgress);
}

/// Dumps run progress with attached extension bytes, corrupts the payload, and
/// records that deserialization fails.
#[when("run progress payload is corrupted")]
fn when_run_progress_payload_corrupted(world: &mut SnapshotExtensionsWorld) {
    let runner = MontyRun::new(world.script.clone(), "test.py", vec![]).expect("runner creation should succeed");
    let progress = runner
        .start(vec![], NoLimitTracker, &mut PrintWriter::Stdout)
        .expect("run should suspend")
        .attach_snapshot_extension(world.snapshot_extension.clone());
    world.load_failed = corrupted_progress_fails_to_load(&progress);
}

/// Starts REPL progress, attaches the test extension bytes, and records the
/// bytes recovered after a dump/load round trip.
#[when("REPL progress is dumped and loaded with snapshot extension bytes")]
fn when_repl_progress_dumped_and_loaded(world: &mut SnapshotExtensionsWorld) {
    let repl = create_repl();
    let progress = repl
        .start(&world.repl_snippet, &mut PrintWriter::Stdout)
        .expect("repl should suspend");
    world.loaded_snapshot_extension = dump_load_round_trip!(progress, world.snapshot_extension.clone(), ReplProgress);
}

/// Dumps REPL progress with attached extension bytes, corrupts the payload, and
/// records that deserialization fails.
#[when("REPL progress payload is corrupted")]
fn when_repl_progress_payload_corrupted(world: &mut SnapshotExtensionsWorld) {
    let repl = create_repl();
    let progress = repl
        .start(&world.repl_snippet, &mut PrintWriter::Stdout)
        .expect("repl should suspend")
        .attach_snapshot_extension(world.snapshot_extension.clone());
    world.load_failed = corrupted_progress_fails_to_load(&progress);
}

/// Verifies that the loaded snapshot-extension bytes match the original input.
#[then("the loaded snapshot extension bytes match")]
fn then_loaded_snapshot_extension_matches(world: &SnapshotExtensionsWorld) {
    assert_eq!(
        world.loaded_snapshot_extension.as_deref(),
        Some(world.snapshot_extension.as_slice()),
        "expected snapshot extension bytes to round-trip"
    );
}

/// Verifies that loading the corrupted run-progress payload failed as expected.
#[then("loading the run progress fails")]
fn then_loading_run_progress_fails(world: &SnapshotExtensionsWorld) {
    assert!(world.load_failed, "expected corrupted payload to fail load");
}

/// Verifies that loading the corrupted REPL-progress payload failed as expected.
#[then("loading the REPL progress fails")]
fn then_loading_repl_progress_fails(world: &SnapshotExtensionsWorld) {
    assert!(world.load_failed, "expected corrupted payload to fail load");
}

/// Runs the BDD scenario that proves run snapshots preserve extension bytes
/// across dump/load.
#[scenario(
    path = "tests/features/snapshot_extensions.feature",
    name = "Run progress preserves snapshot extension bytes across dump/load"
)]
fn run_snapshot_extension_round_trip(world: SnapshotExtensionsWorld) {
    drop(world);
}

/// Runs the BDD scenario that expects corrupted run-progress payloads to fail
/// deserialization.
#[scenario(
    path = "tests/features/snapshot_extensions.feature",
    name = "Corrupted run progress payload fails to load"
)]
fn corrupted_run_progress_payload(world: SnapshotExtensionsWorld) {
    drop(world);
}

/// Runs the BDD scenario that expects corrupted REPL-progress payloads to fail
/// deserialization.
#[scenario(
    path = "tests/features/snapshot_extensions.feature",
    name = "Corrupted REPL progress payload fails to load"
)]
fn corrupted_repl_progress_payload(world: SnapshotExtensionsWorld) {
    drop(world);
}

/// Runs the BDD scenario that proves REPL snapshots preserve extension bytes
/// across dump/load.
#[scenario(
    path = "tests/features/snapshot_extensions.feature",
    name = "REPL progress preserves snapshot extension bytes across dump/load"
)]
fn repl_snapshot_extension_round_trip(world: SnapshotExtensionsWorld) {
    drop(world);
}
