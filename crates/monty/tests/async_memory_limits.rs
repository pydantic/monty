//! Tests that the async runtime accounts its dynamically-allocated state
//! against `ResourceTracker`, so a configured `max_memory` actually
//! bounds gathers over many unresolved externals and unbounded async
//! recursion via `gather`.

use std::{mem, rc::Rc, time::Duration};

use monty::{MontyRun, RunProgress};
use monty_types::{
    CompileOptions, ExcType, ExtFunctionResult, LimitedTracker, MontyException, MontyObject, NameLookupResult,
    PrintWriter, ResourceError, ResourceLimits, ResourceTracker,
};

/// Wraps `LimitedTracker` in `Rc` so a test can hold its own handle for
/// probing `current_memory()` while the VM owns one for accounting.
#[derive(Debug, Clone)]
struct SharedTracker(Rc<LimitedTracker>);

impl SharedTracker {
    fn new(limits: ResourceLimits) -> Self {
        Self(Rc::new(LimitedTracker::new(limits)))
    }

    fn current_memory(&self) -> usize {
        self.0.current_memory()
    }
}

impl ResourceTracker for SharedTracker {
    fn on_free(&self, get_size: impl FnOnce() -> usize) {
        self.0.on_free(get_size);
    }

    fn check_time(&self) -> Result<(), ResourceError> {
        self.0.check_time()
    }

    fn check_recursion_depth(&self, current_depth: usize) -> Result<(), ResourceError> {
        self.0.check_recursion_depth(current_depth)
    }

    fn check_large_result(&self, estimated_bytes: usize) -> Result<(), ResourceError> {
        self.0.check_large_result(estimated_bytes)
    }

    fn on_grow(&self, get_additional: impl FnOnce() -> usize) -> Result<(), ResourceError> {
        self.0.on_grow(get_additional)
    }

    fn gc_interval(&self) -> Option<usize> {
        self.0.gc_interval()
    }

    fn on_execution_start(&self) {
        self.0.on_execution_start();
    }

    fn on_execution_stop(&self) {
        self.0.on_execution_stop();
    }
}

/// Drives `RunProgress` past every `NameLookup` and every `FunctionCall`
/// (treating each external call as still pending — the host never
/// resolves them). Returns whatever non-name/non-call state the VM
/// settles into, or the exception it raises along the way.
///
/// Used by the gather bookkeeping witness, which expects the run to
/// raise `MemoryError` inside the gather await *before* it would
/// otherwise settle at `ResolveFutures`.
fn drive_until_settled<T: ResourceTracker>(mut progress: RunProgress<T>) -> Result<RunProgress<T>, MontyException> {
    loop {
        match progress {
            RunProgress::NameLookup(lookup) => {
                let name = lookup.name.clone();
                progress = lookup.resume(
                    NameLookupResult::Value(MontyObject::Function { name, docstring: None }),
                    PrintWriter::Stdout,
                )?;
            }
            RunProgress::FunctionCall(call) => {
                progress = call.resume_pending(PrintWriter::Stdout)?;
            }
            other => return Ok(other),
        }
    }
}

/// Builds top-level code that awaits `asyncio.gather(*pendings)` over a
/// list of `n` unresolved external futures. Splatting through `*args`
/// dodges the 255-argument literal limit Monty inherits from CPython
/// while still producing the same Pending → Awaited transition with
/// `n` slots in `results` and `n` entries in `pending_children`.
fn gather_n_pending_runner(n: usize) -> MontyRun {
    let code = format!(
        r"
import asyncio

async def main():
    pendings = [pending() for _ in range({n})]
    await asyncio.gather(*pendings)

await main()
"
    );
    MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap()
}

/// `await_gather_future` must charge its per-await bookkeeping
/// (`pending_children` + `results`) against the tracker.
///
/// Run with a generous budget so the gather drives to `ResolveFutures`
/// without raising; then assert `current_memory()` includes the
/// bookkeeping. A budget-based witness is unreliable: transient
/// allocations between list construction and gather construction
/// exceed any threshold sitting close to the bookkeeping delta. For
/// N = 10_000 unresolved externals the threshold (1.25 MiB) sits
/// between the pre-fix counter (~1.12 MiB) and the post-fix counter
/// (~1.36 MiB).
#[test]
fn gather_awaited_state_charged_against_tracker() {
    let n = 10_000;
    let runner = gather_n_pending_runner(n);

    let limits = ResourceLimits::new()
        .max_memory(10 * 1024 * 1024)
        .max_duration(Duration::from_secs(30));
    let tracker = SharedTracker::new(limits);
    let handle = tracker.clone();
    let progress = runner.start(vec![], tracker, PrintWriter::Stdout).unwrap();
    let settled = drive_until_settled(progress).expect("run must reach ResolveFutures without raising");
    let resolve = match settled {
        RunProgress::ResolveFutures(state) => state,
        other => panic!(
            "expected the run to suspend at ResolveFutures after building the gather (got {:?})",
            mem::discriminant(&other),
        ),
    };

    let memory = handle.current_memory();
    let post_fix_threshold = 1_250_000;
    let threshold_failure = (memory < post_fix_threshold).then_some(memory);

    // Tear the gather down before dropping the snapshot. Dropping
    // `ResolveFutures` directly auto-drops `Value::Ref`s on the saved
    // VM stack, which `memory-model-checks` treats as a refcounting bug.
    let first_call = resolve.pending_call_ids()[0];
    let error = MontyException::new(ExcType::ValueError, Some("test-shutdown".to_string()));
    let _ = resolve.resume(vec![(first_call, ExtFunctionResult::Error(error))], PrintWriter::Stdout);

    if let Some(memory) = threshold_failure {
        panic!(
            "Awaited bookkeeping not charged: tracker memory = {memory} bytes; \
             expected at least {post_fix_threshold} for N = {n}.",
        );
    }
}

/// `deep(n)` that returns the gather result back through every level
/// used to trip a "no active frame" panic when the memory budget fired
/// mid-recursion inside `save_task_context` after it had already
/// drained `self.frames`. Charging the tracker before the drain — plus
/// a `current_frame_name` fallback when frames is empty — turns this
/// back into a graceful `MemoryError`.
#[test]
fn recursive_gather_with_return_value_hits_memory_limit_not_panic() {
    let code = r"
import asyncio

async def deep(n):
    if n <= 0:
        return None
    r = await asyncio.gather(deep(n - 1))
    return r[0]

asyncio.run(deep(20000))
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let limits = ResourceLimits::new()
        .max_memory(128 * 1024)
        .max_duration(Duration::from_secs(30));
    let tracker = LimitedTracker::new(limits);
    let result = runner.run(vec![], tracker, PrintWriter::Stdout);

    let exc = result.expect_err("deep recursive gather must be bounded by the memory limit");
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    let msg = exc.message().expect("memory error carries a message");
    assert!(
        msg.starts_with("memory limit exceeded:"),
        "expected memory-limit error from scheduler task accounting: {msg}"
    );
}

/// Unbounded `async def f(): await asyncio.gather(f())` must terminate
/// under any configured `max_memory` instead of growing the worker
/// until the system allocator aborts.
#[test]
fn recursive_gather_hits_memory_limit_not_sigabrt() {
    let code = r"
import asyncio

async def f():
    return await asyncio.gather(f())

asyncio.run(f())
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let limits = ResourceLimits::new()
        .max_memory(128 * 1024)
        .max_duration(Duration::from_secs(30));
    let tracker = LimitedTracker::new(limits);
    let result = runner.run(vec![], tracker, PrintWriter::Stdout);

    let exc = result.expect_err("recursive gather must be bounded by the memory limit");
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    let msg = exc.message().expect("memory error carries a message");
    assert!(
        msg.starts_with("memory limit exceeded:"),
        "expected memory-limit error from scheduler task accounting: {msg}"
    );
}
