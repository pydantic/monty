//! Tests that the async runtime accounts its dynamically-allocated state
//! against `ResourceTracker`.
//!
//! Two memory-bypass classes are covered here. Both let untrusted Monty code
//! grow live memory inside the worker without `LimitedTracker::max_memory`
//! ever seeing it, which in the worst case drives the host process to
//! allocator abort (SIGABRT) — see `security-reports/async-memory.md` and
//! the SIGABRT group in `security-reports/subprocess_panics.md`.
//!
//! 1. **Gather `Pending → Awaited` bookkeeping.** `GatherFuture` is created
//!    in `GatherState::Pending`, whose `py_estimate_size` charges no
//!    state-side bytes. On first await, `await_gather_future` allocates
//!    `pending_children` and `results` slot storage and stores them as
//!    `GatherState::Awaited(...)`. That growth must be charged against
//!    the tracker; otherwise an `await asyncio.gather(...)` over many
//!    unresolved externals keeps O(N) untracked bytes live.
//!
//! 2. **Scheduler task overhead.** `Scheduler::spawn` and
//!    `VM::save_task_context` allocate a `Task` (plus map entries and
//!    a saved frames/stack/exception_stack) on every coroutine spawn and
//!    suspension. None of that is charged today, so deeply recursive
//!    `await asyncio.gather(f())` consumes host memory unbounded.

use std::{mem, rc::Rc, time::Duration};

use monty::{
    ExcType, ExtFunctionResult, LimitedTracker, MontyException, MontyObject, MontyRun, NameLookupResult, PrintWriter,
    ResourceError, ResourceLimits, ResourceTracker, RunProgress,
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
    fn on_allocate(&self, get_size: impl FnOnce() -> usize) -> Result<(), ResourceError> {
        self.0.on_allocate(get_size)
    }

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

    fn on_grow(&self, additional_bytes: usize) -> Result<(), ResourceError> {
        self.0.on_grow(additional_bytes)
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
fn drive_until_settled<T: monty::ResourceTracker>(
    mut progress: RunProgress<T>,
) -> Result<RunProgress<T>, monty::MontyException> {
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
    MontyRun::new(code, "test.py", vec![]).unwrap()
}

// ============================================================================
// Bug A — gather Pending → Awaited bookkeeping is charged against the tracker.
// ============================================================================

/// Witness for the bypass reported in `security-reports/async-memory.md`.
///
/// With `n` unresolved externals the Awaited-state bookkeeping is on the
/// order of `n * (sizeof::<Option<Value>>() + sizeof::<HeapId>())` bytes
/// (`results` slot vector + `pending_children` map entries — see
/// `heap_data.rs::py_estimate_size for GatherFuture`). The chosen budget
/// covers the bare GatherFuture, the `n` `ExternalFuture` heap entries,
/// the coroutine frame, and module bookkeeping, but leaves no headroom
/// for the per-await bookkeeping. Post-fix the `Pending → Awaited`
/// transition raises `MemoryError`; pre-fix the run quietly settles into
/// `ResolveFutures` with the Awaited bookkeeping off-the-books.
#[test]
fn gather_awaited_state_charged_against_tracker() {
    // Run with a generous budget so the gather drives all the way to
    // `ResolveFutures`. Once there, tracker `current_memory()` must
    // include the bookkeeping that `await_gather_future` just stored
    // (`pending_children` map + `results` slot vector). Pre-fix the
    // transition didn't call `track_growth`, so the tracker counter
    // was about 1.12 MiB at this point; post-fix it is about
    // 1.36 MiB — the ~240 KiB Awaited state-size for N = 10_000.
    //
    // A budget-based witness is unreliable because the run has
    // transient allocation spikes between the list comprehension and
    // the gather construction that exceed any threshold sitting
    // close to the bookkeeping delta. Observing the post-await
    // tracker counter directly is the cleanest signal.
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
    // Pre-fix measured: ~1_120_448 bytes (no charge for `pending_children`
    // or `results`). Post-fix: ~1_360_448 bytes. The threshold sits
    // safely between, so it only holds when the per-await bookkeeping
    // is accounted.
    let post_fix_threshold = 1_250_000;
    let threshold_failure = (memory < post_fix_threshold).then_some(memory);

    // Resume with an error on the first pending external so the gather
    // tears down and the snapshot is consumed cleanly. Dropping a
    // `ResolveFutures` directly would leave `Value::Ref` entries inside
    // `VMSnapshot.stack` to be auto-dropped, which `memory-model-checks`
    // (correctly) treats as a refcounting bug.
    let first_call = resolve.pending_call_ids()[0];
    let error = MontyException::new(ExcType::ValueError, Some("test-shutdown".to_string()));
    let outcome = resolve.resume(vec![(first_call, ExtFunctionResult::Error(error))], PrintWriter::Stdout);
    // The gather propagates the error; either form is acceptable —
    // what matters is that the heap is consumed and freed cleanly.
    let _ = outcome;

    if let Some(memory) = threshold_failure {
        panic!(
            "Gather Pending → Awaited bookkeeping is not charged against \
             the tracker: tracker memory = {memory} bytes; expected at \
             least {post_fix_threshold} (pre-fix is ~1.12 MiB, post-fix \
             ~1.36 MiB for N = {n}).",
        );
    }
}

// ============================================================================
// Bug B — Scheduler task overhead must not SIGABRT under a memory cap.
// ============================================================================

/// Regression for the SIGABRT case from
/// `security-reports/subprocess_panics.md` (group 4, snippet 51165).
///
/// `async def f(): return await asyncio.gather(f())` creates one new
/// scheduler task per recursion level. Without tracker accounting on
/// `Scheduler::spawn` and `VM::save_task_context`, the worker grows
/// linearly per level and is eventually killed by the system
/// allocator. With *any* memory cap configured, this run MUST exit
/// gracefully with `MemoryError` rather than running the host out of
/// memory — the tracked Coroutine + GatherFuture allocations alone
/// are enough to trip the budget today, but the test also pins that
/// the additional scheduler-task accounting (added by Bug B's fix)
/// cannot regress the worker into the SIGABRT path.
#[test]
fn recursive_gather_hits_memory_limit_not_sigabrt() {
    let code = r"
import asyncio

async def f():
    return await asyncio.gather(f())

asyncio.run(f())
";
    let runner = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new()
        .max_memory(128 * 1024)
        .max_allocations(50_000)
        .max_duration(Duration::from_secs(30));
    let tracker = LimitedTracker::new(limits);
    let result = runner.run(vec![], tracker, PrintWriter::Stdout);

    let exc = result.expect_err("recursive gather must be bounded by the memory limit");
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    let msg = exc.message().expect("memory error carries a message");
    assert!(
        msg.starts_with("memory limit exceeded:"),
        "expected memory-limit error from scheduler task accounting, \
         not the allocation-count safety net: {msg}"
    );
}
