/// Tests for execution-time limits and garbage collection.
///
/// Allocator-backed memory limits are exercised through worker subprocesses.
use std::{
    thread,
    time::{Duration, Instant},
};

use monty::{MontyRepl, MontyRun, RunProgress};
use monty_types::{
    CompileOptions, ExcType, MontyException, MontyObject, NameLookupResult, PrintWriter, ResourceLimits,
    ResourceTracker,
};

/// Resolves consecutive `NameLookup` yields by providing a `Function` object for each name.
///
/// External functions are no longer declared upfront. Instead, the VM yields `NameLookup`
/// when it encounters an unresolved name. This helper resolves all such lookups until
/// a different progress variant is reached.
fn resolve_name_lookups(mut progress: RunProgress) -> Result<RunProgress, MontyException> {
    while let RunProgress::NameLookup(lookup) = progress {
        let name = lookup.name.clone();
        progress = lookup.resume(
            NameLookupResult::Value(MontyObject::Function { name, docstring: None }),
            PrintWriter::Stdout,
        )?;
    }
    Ok(progress)
}

/// Test that GC properly collects dict cycles.
///
/// Each iteration creates a fresh `d1 <-> d2` cycle and the next iteration's
/// reassignment leaves it unreachable. Trial deletion enrolls those entries
/// as cycle-root candidates via `dec_ref`; the alloc-count interval is what
/// actually fires the collector at a controlled rate.
#[test]
#[cfg(feature = "ref-count-return")]
fn gc_collects_dict_cycles_via_has_refs() {
    // Create 200,001 dict cycles. Each iteration allocates two GC-tracked
    // dicts and forms a cycle between them; on the next iteration, both are
    // reassigned and the cycle is unreachable.
    //
    // GC fires every DEFAULT_GC_INTERVAL (100,000) GC-tracked allocations
    // when there are pending cycle candidates. With ~400k allocations across
    // 200,001 iterations, the collector must run at least once.
    let code = r"
# Create many dict cycles
for i in range(200001):
    d1 = {}
    d2 = {'ref': d1}
    d1['ref'] = d2    # Cycle formed; reassignment next iteration seeds the GC

# Create final result (not a cycle)
result = 'done'
result
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let output = ex.run_ref_counts(vec![]).expect("should succeed");

    // DEFAULT_GC_INTERVAL is 100,000. With 200,001 iterations creating dict
    // cycles, GC must have run at least once, resetting allocations_since_gc.
    // If the collector never ran, allocations_since_gc would be ~400k
    // (2 dicts per iteration).
    assert!(
        output.allocations_since_gc < 100_000,
        "GC should have run: allocations_since_gc = {}",
        output.allocations_since_gc
    );

    // Verify that GC collected most cycles.
    // If GC failed to collect cycles, heap_count would be >> 400k.
    // We allow a small number of extra objects for implementation details.
    assert!(
        output.heap_count < 20,
        "GC should collect most unreachable dict cycles: {} heap objects (expected < 20)",
        output.heap_count
    );
}

/// Test that GC collects cycles between lists and their iterators.
#[test]
#[cfg(feature = "ref-count-return")]
fn gc_collects_list_iterator_cycles() {
    let code = r"
for i in range(100001):
    a = []
    iterator = iter(a)
    a.append(iterator)

result = [1, 2, 3]
len(result)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let output = ex.run_ref_counts(vec![]).expect("should succeed");

    assert!(
        output.heap_count < 30,
        "GC should collect list-iterator cycles: {} heap objects (expected < 30)",
        output.heap_count
    );
}

/// Test that GC traces sources retained by tuple and dictionary iterators.
#[test]
#[cfg(feature = "ref-count-return")]
fn gc_collects_concrete_iterator_cycles() {
    let code = r"
for i in range(100001):
    container = []
    source = (container,)
    iterator = iter(source)
    container.append(iterator)

    mapping = {}
    iterator = iter(mapping)
    mapping['iterator'] = iterator

result = [1, 2, 3]
len(result)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let output = ex.run_ref_counts(vec![]).expect("should succeed");

    assert!(
        output.heap_count < 40,
        "GC should collect concrete iterator cycles: {} heap objects (expected < 40)",
        output.heap_count
    );
}

/// Cycles through `callable_iterator` / `list_iterator` must be collected even
/// when the iterator is the last external reference dropped.
#[test]
#[cfg(feature = "ref-count-return")]
fn gc_collects_iterator_cycles_rooted_by_the_iterator() {
    let code = r"
class Src:
    def step(self):
        return 1

roots = []
for i in range(2000):
    o = Src()
    it = iter(o.step, 0)
    o.it = it
    roots.append(it)

    a = []
    li = iter(a)
    a.append(li)
    roots.append(li)

roots = None

for i in range(2000):
    d = {}
    d['self'] = d

result = 'done'
result
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let tracker = ResourceTracker::new(ResourceLimits::default().gc_interval(500));
    let output = ex.run_ref_counts_with_tracker(vec![], tracker).expect("should succeed");

    assert!(
        output.heap_count < 20,
        "GC should collect iterator-rooted cycles: {} heap objects (expected < 20)",
        output.heap_count
    );
}

/// Test that GC properly collects self-referencing list cycles.
///
/// Each iteration's `a.append(a)` produces a self-referencing list; the next
/// iteration's reassignment leaves the previous list unreachable. Trial
/// deletion enrolls it as a candidate via `dec_ref`, and the alloc-count
/// interval triggers the collector once enough have accumulated.
#[test]
#[cfg(feature = "ref-count-return")]
fn gc_collects_list_cycles() {
    // Create 200,001 self-referencing list cycles. Each iteration:
    // - Creates empty list `a`
    // - Appends `a` to itself (creating a self-reference cycle)
    // - On next iteration, `a` is reassigned, making the cycle unreachable
    //
    // GC fires every DEFAULT_GC_INTERVAL (100,000) GC-tracked allocations
    // when there are pending candidates. With 200,001 iterations the
    // collector must run at least twice. After it runs, only the final
    // cycle should remain.
    let code = r"
# Create many self-referencing list cycles
for i in range(200001):
    a = []
    a.append(a)  # Creates cycle; reassignment next iteration seeds the GC

# Create final result (not a cycle)
result = [1, 2, 3]
len(result)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let output = ex.run_ref_counts(vec![]).expect("should succeed");

    // DEFAULT_GC_INTERVAL is 100,000. With 200,001 iterations creating list
    // cycles, GC must have run at least twice, resetting allocations_since_gc.
    assert!(
        output.allocations_since_gc < 100_000,
        "GC should have run: allocations_since_gc = {}",
        output.allocations_since_gc
    );

    // Verify that GC collected most cycles.
    // If GC failed to collect cycles, heap_count would be >> 200k.
    assert!(
        output.heap_count < 20,
        "GC should collect most unreachable list cycles: {} heap objects (expected < 20)",
        output.heap_count
    );

    // Verify expected ref counts
    // `a` is the last self-referencing list (refcount 2: variable + self-reference)
    // `result` is a simple list (refcount 1: just the variable)
    assert_eq!(
        output.counts.get("a"),
        Some(&2),
        "self-referencing list should have refcount 2"
    );
    assert_eq!(
        output.counts.get("result"),
        Some(&1),
        "result list should have refcount 1"
    );
}

#[test]
fn time_limit_exceeded() {
    // Create a long-running loop using for + range (while isn't implemented yet)
    // Use a very large range to ensure it runs long enough to hit the time limit
    let code = r"
x = 0
for i in range(100000000):
    x = x + 1
x
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    // Set a short time limit
    let limits = ResourceLimits::default().max_duration(Duration::from_millis(50));
    let result = ex.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout);

    // Should fail due to time limit
    assert!(result.is_err(), "should exceed time limit");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
    assert!(
        exc.message().is_some_and(|m| m.contains("time limit exceeded")),
        "expected time limit error, got: {exc}"
    );
}

#[test]
fn time_limit_not_exceeded() {
    // Simple code that runs quickly
    let code = "x = 1 + 2\nx";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    // Set a generous time limit
    let limits = ResourceLimits::default().max_duration(Duration::from_secs(5));
    let result = ex.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout);

    // Should succeed
    assert!(result.is_ok(), "should not exceed time limit");
}

#[test]
fn run_without_limits_succeeds() {
    // Verify that run() still works (no limits)
    let code = r"
result = []
for i in range(100):
    result.append(str(i))
len(result)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    // Standard run should succeed
    let result = ex.run_no_limits(vec![]);
    assert!(result.is_ok(), "standard run should succeed");
}

#[test]
#[cfg(feature = "ref-count-return")]
fn gc_interval_triggers_collection() {
    // This test verifies that the built-in GC interval still triggers
    // collection on real reference cycles even when no custom tracker
    // interval is supplied. A sufficiently large number of cycles forces
    // collection here.
    let code = r"
result = 'done'
for i in range(210000):
    a = []
    a.append(a)
result
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let output = ex
        .run_ref_counts(vec![])
        .expect("should succeed with GC enabled on cycles");

    assert_eq!(output.py_object, MontyObject::String("done".to_owned()));
    assert!(
        output.allocations_since_gc < 100_000,
        "default GC interval should have triggered collection: allocations_since_gc = {}",
        output.allocations_since_gc
    );
    // Expected remaining cycles × 2, with a little slack.
    assert!(
        output.heap_count <= 20_000,
        "GC should collect most unreachable list cycles: {} heap objects",
        output.heap_count
    );
}

#[test]
#[cfg(feature = "ref-count-return")]
fn gc_interval_limit_is_respected() {
    // This test verifies that a custom GC interval is actually used instead
    // of the built-in default. We create self-referencing list cycles so GC
    // is eligible to run, then assert that a small configured interval
    // causes a collection before the default 100,000-allocation threshold.
    let code = r"
for i in range(25):
    a = []
    a.append(a)
result = 'done'
result
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let limits = ResourceLimits::default().gc_interval(10);
    let output = ex
        .run_ref_counts_with_tracker(vec![], ResourceTracker::new(limits))
        .expect("should succeed with custom GC interval");

    assert_eq!(output.py_object, MontyObject::String("done".to_owned()));
    assert!(
        output.allocations_since_gc < 10,
        "configured GC interval should trigger collections before the default; allocations_since_gc = {}",
        output.allocations_since_gc
    );
    // Expected remaining cycles × 2, with a little slack.
    assert!(
        output.heap_count <= 10,
        "GC should collect most unreachable list cycles: {} heap objects",
        output.heap_count
    );
}

// === Timeout enforcement in builtin iteration loops ===
// These tests verify that `max_duration_secs` is enforced inside Rust-side loops
// within builtin functions. Previously, builtins like sum(), sorted(), min(), max()
// ran Rust loops entirely within a single bytecode instruction, bypassing the VM's
// per-instruction timeout check. The fix adds `heap.check_time()` calls inside
// Python iterator advancement and other non-iterator loops.

/// Helper: runs code with a short time limit and asserts it produces a TimeoutError promptly.
fn assert_timeout_in_builtin(code: &str, label: &str) {
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let limits = ResourceLimits::default().max_duration(Duration::from_millis(100));
    let start = Instant::now();
    let result = ex.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout);
    let elapsed = start.elapsed();

    assert!(result.is_err(), "{label}: should exceed time limit");
    let exc = result.unwrap_err();
    assert_eq!(
        exc.exc_type(),
        ExcType::TimeoutError,
        "{label}: expected TimeoutError, got: {exc}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "{label}: should terminate promptly, took {elapsed:?}"
    );
}

/// Test that `sum(range(huge))` respects the time limit.
///
/// `sum()` iterates via `for_next()` which now calls `heap.check_time()`.
#[test]
fn timeout_in_sum_builtin() {
    assert_timeout_in_builtin("sum(range(10**18))", "sum(range(10**18))");
}

/// Test that `list(range(huge))` respects the time limit.
///
/// The `list()` constructor drains its concrete Python iterator.
#[test]
fn timeout_in_list_constructor() {
    assert_timeout_in_builtin("list(range(10**18))", "list(range(10**18))");
}

/// Covers all four substring scanners; `index`/`rindex` share theirs with
/// `find`/`rfind`.
const BYTES_SEARCH_EXPRS: &[&str] = &[
    "needle in haystack",
    "haystack.find(needle)",
    "haystack.rfind(needle)",
    "haystack.count(needle)",
    "haystack.split(needle)",
    "haystack.rsplit(needle)",
    "haystack.replace(needle, b'')",
    "haystack.partition(needle)",
    "haystack.rpartition(needle)",
];

/// Runs `expr` with `haystack`/`needle` bound, under `limits`.
fn run_bytes_search(expr: &str, haystack: Vec<u8>, needle: Vec<u8>, limits: ResourceLimits) -> BytesSearchOutcome {
    let run = MontyRun::new(
        expr.to_owned(),
        "test.py",
        vec!["haystack".to_owned(), "needle".to_owned()],
        CompileOptions::default(),
    )
    .unwrap();

    let start = Instant::now();
    let result = run.run(
        vec![MontyObject::Bytes(haystack), MontyObject::Bytes(needle)],
        ResourceTracker::new(limits),
        PrintWriter::Stdout,
    );
    BytesSearchOutcome {
        elapsed: start.elapsed(),
        result,
    }
}

/// What a `bytes` search returned, and how long it took.
struct BytesSearchOutcome {
    elapsed: Duration,
    result: Result<MontyObject, MontyException>,
}

/// Worst case for a naive `windows()` scan: every offset compares the full
/// needle before failing on its last byte.
fn near_match_inputs(haystack_len: usize, needle_len: usize) -> (Vec<u8>, Vec<u8>) {
    let mut needle = vec![b'a'; needle_len];
    *needle.last_mut().unwrap() = b'b';
    (vec![b'a'; haystack_len], needle)
}

/// Near-matching probes must not blow up quadratically.
///
/// A naive scan takes ~800ms on these inputs, a linear one microseconds; the
/// generous budget keeps the timing assertion robust on loaded CI.
#[test]
fn bytes_search_is_not_quadratic() {
    for expr in BYTES_SEARCH_EXPRS {
        let (haystack, needle) = near_match_inputs(1_000_000, 50_000);
        let outcome = run_bytes_search(expr, haystack, needle, ResourceLimits::default());

        assert!(
            outcome.result.is_ok(),
            "{expr}: expected success, got {:?}",
            outcome.result
        );
        assert!(
            outcome.elapsed < Duration::from_millis(300),
            "{expr}: took {:?}, expected a linear scan",
            outcome.elapsed
        );
    }
}

/// Bytes searches must remain interruptible by the time limit.
///
/// The haystack is large enough that even a linear scan outlives the budget.
#[test]
fn timeout_in_bytes_search() {
    for expr in BYTES_SEARCH_EXPRS {
        let (haystack, needle) = near_match_inputs(64 * 1024 * 1024, 4096);
        let limits = ResourceLimits::default().max_duration(Duration::from_millis(1));
        let outcome = run_bytes_search(expr, haystack, needle, limits);

        let exc = outcome
            .result
            .expect_err(&format!("{expr}: expected the time limit to fire"));
        assert_eq!(exc.exc_type(), ExcType::TimeoutError, "{expr}");
        assert!(
            outcome.elapsed < Duration::from_secs(2),
            "{expr}: should terminate promptly, took {:?}",
            outcome.elapsed
        );
    }
}

/// Test that a bounded `deque * n` repetition respects the time limit mid-build.
///
/// `repeat_deque` clones into a Rust-side loop that polls `check_time()`. Beyond
/// enforcing the limit, a timeout must release the clones built so far — the
/// heap-ref element makes a leak observable (it panics under memory-model-checks).
#[test]
fn timeout_in_bounded_deque_repeat() {
    assert_timeout_in_builtin(
        "from collections import deque\ndeque([[1]], maxlen=10**9) * 10**9",
        "deque(maxlen=10**9) * 10**9",
    );
}

/// Test that `sorted(range(huge))` respects the time limit.
///
/// `sorted()` first collects items via `for_next()`, then sorts. The collection
/// phase alone should trigger the timeout for very large ranges.
#[test]
fn timeout_in_sorted_builtin() {
    assert_timeout_in_builtin("sorted(range(10**18))", "sorted(range(10**18))");
}

/// Test that `min(range(huge))` respects the time limit.
///
/// `min()` with a single iterable argument iterates via `for_next()`.
#[test]
fn timeout_in_min_builtin() {
    assert_timeout_in_builtin("min(range(10**18))", "min(range(10**18))");
}

/// Test that `max(range(huge))` respects the time limit.
///
/// `max()` with a single iterable argument iterates via `for_next()`.
#[test]
fn timeout_in_max_builtin() {
    assert_timeout_in_builtin("max(range(10**18))", "max(range(10**18))");
}

/// Test that `all(range(huge))` respects the time limit.
///
/// `all()` iterates via `for_next()` and only short-circuits on falsy values.
/// `range(1, 10**18)` produces only truthy values so it keeps iterating.
#[test]
fn timeout_in_all_builtin() {
    assert_timeout_in_builtin("all(range(1, 10**18))", "all(range(1, 10**18))");
}

/// Test that `enumerate(range(huge))` iteration respects the time limit.
///
/// `enumerate()` creates tuples on each iteration via `for_next()`.
#[test]
fn timeout_in_any_builtin() {
    // range(0, 1) repeated via a for loop calling any on each chunk isn't ideal,
    // but we can test with a large range starting from 0 where only first element is falsy
    // Actually, any(range(10**18)) will return True immediately because range starts at 0
    // which is falsy, but 1 is truthy. So any() returns True after checking 0, 1.
    // Instead, we need a different approach - just use the for_next timeout via enumerate.
    assert_timeout_in_builtin("list(enumerate(range(10**18)))", "enumerate(range(10**18))");
}

/// Test that `tuple(range(huge))` respects the time limit.
///
/// The `tuple()` constructor drains its concrete Python iterator.
#[test]
fn timeout_in_tuple_constructor() {
    assert_timeout_in_builtin("tuple(range(10**18))", "tuple(range(10**18))");
}

/// Test that `' '.join(...)` iteration respects the time limit.
///
/// `str.join()` collects items from the iterable via `for_next()`.
#[test]
fn timeout_in_str_join() {
    assert_timeout_in_builtin("' '.join(str(i) for i in range(10**18))", "str.join with generator");
}

/// Test that the insertion sort inner loop in `sorted()` respects the time limit.
///
/// Uses reverse-sorted data to trigger worst-case O(n^2) insertion sort behavior.
/// The sort comparison loop has an explicit `heap.check_time()` call.
#[test]
fn timeout_in_sorted_comparison_loop() {
    // Build a reverse-sorted list, then sort it. Insertion sort on reverse-sorted
    // data is O(n^2).
    let code = r"
x = list(range(10**6, 0, -1))
sorted(x)
";
    assert_timeout_in_builtin(code, "sorted(reversed list)");
}

/// Test that `[1] * 10_000_000` (list repetition) respects the time limit.
///
/// The sequence-repetition copy loop in `py_mult` now calls `heap.check_time()`
/// on each repetition to prevent large sequence multiplications from bypassing timeout.
#[test]
fn timeout_in_list_repetition() {
    assert_timeout_in_builtin("[1, 2, 3] * 10_000_000", "list repetition");
}

/// Test that `(1,) * 10_000_000` (tuple repetition) respects the time limit.
///
/// Same as list repetition but for tuples — both sequence-repetition paths in
/// `py_mult` now check the time limit.
#[test]
fn timeout_in_tuple_repetition() {
    assert_timeout_in_builtin("(1, 2, 3) * 10_000_000", "tuple repetition");
}

/// Test that comparing two large equal lists respects the time limit.
///
/// `List::py_eq_impl()` iterates element-wise comparing pairs. With large equal lists,
/// it must compare every element before returning True.
#[test]
fn timeout_in_list_equality() {
    let code = r"
a = list(range(10_000_000))
b = list(range(10_000_000))
a == b
";
    assert_timeout_in_builtin(code, "list equality");
}

/// Test that comparing two large equal dicts respects the time limit.
///
/// `Dict::py_eq_impl()` iterates all entries checking keys and values. With large equal
/// dicts, it must check every entry before returning True.
#[test]
fn timeout_in_dict_equality() {
    let code = r"
a = {i: i for i in range(10_000_000)}
b = {i: i for i in range(10_000_000)}
a == b
";
    assert_timeout_in_builtin(code, "dict equality");
}

/// Test that `str.splitlines()` on a large string respects the time limit.
///
/// `str_splitlines()` scans the entire string for line endings in a while loop
/// that now calls `heap.check_time()` on each iteration.
#[test]
fn timeout_in_str_splitlines() {
    let code = r"
s = 'a\n' * 5_000_000
s.splitlines()
";
    assert_timeout_in_builtin(code, "str.splitlines()");
}

/// Test that `bytes.splitlines()` on large bytes respects the time limit.
///
/// `bytes_splitlines()` scans bytes for line endings and now checks the time limit.
#[test]
fn timeout_in_bytes_splitlines() {
    let code = r"
s = b'a\n' * 5_000_000
s.splitlines()
";
    assert_timeout_in_builtin(code, "bytes.splitlines()");
}

// === Timeout truncation in repr ===
// These tests verify that `repr()` on large containers respects the time limit
// and terminates promptly instead of hanging indefinitely. The repr methods
// (`repr_sequence_fmt`, `Dict::py_repr_fmt`, `SetInner::repr_fmt`) call
// `heap.check_time()` on each iteration and write `...[timeout]` when the
// time limit is exceeded, returning normally instead of propagating an error.
//
// Each test uses the external function "interrupt" pattern: the large object is
// built with NO time limit, then execution pauses at `interrupt()`. A short time
// limit is set before resuming, so only the `repr()` call is timed.

/// The `max_duration` clock measures cumulative *execution* time only: time
/// spent suspended at an external call must not consume the budget. Here the
/// host stays away for 3× the entire budget while the sandbox is suspended,
/// and execution still completes — under the old wall-clock-since-creation
/// accounting this raised TimeoutError on resume.
#[test]
fn suspension_time_does_not_count_toward_max_duration() {
    let code = "interrupt()\nsum(range(100))";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
    let limits = ResourceLimits::default().max_duration(Duration::from_millis(100));
    let progress = run
        .start(vec![], ResourceTracker::new(limits), PrintWriter::Stdout)
        .unwrap();
    let call = resolve_name_lookups(progress)
        .unwrap()
        .into_function_call()
        .expect("interrupt call");

    thread::sleep(Duration::from_millis(300));

    let progress = call.resume(MontyObject::None, PrintWriter::Stdout).unwrap();
    let RunProgress::Complete(value) = progress else {
        panic!("expected Complete, got another suspension");
    };
    assert_eq!(value, MontyObject::Int(4950));
}

/// `MontyRepl::call_function` is a host boundary like `feed_run`: it must
/// open an execution window so the cumulative `max_duration` clock advances
/// during the call. With the window left closed, `elapsed()` is frozen and an
/// infinite loop in the called function would run forever.
#[test]
fn call_function_enforces_max_duration() {
    let limits = ResourceLimits::default().max_duration(Duration::from_millis(50));
    let mut repl = MontyRepl::new("test.py", ResourceTracker::new(limits), CompileOptions::default());
    repl.feed_run(
        "def spin():\n    while True:\n        pass",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    let exc = repl
        .call_function("spin", vec![], PrintWriter::Stdout)
        .expect_err("infinite loop must hit the time limit");
    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
}

/// Helper: builds a large object without time limit, then runs `repr()` on it
/// with a short time limit and asserts it produces a TimeoutError promptly.
///
/// The code must call `interrupt()` between object construction and `repr()`.
fn assert_repr_timeout(code: &str, label: &str) {
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    // Phase 1: build the large object with no time limit
    let limits = ResourceLimits::default();
    let progress = run
        .start(vec![], ResourceTracker::new(limits), PrintWriter::Stdout)
        .unwrap();
    let mut call = resolve_name_lookups(progress)
        .unwrap()
        .into_function_call()
        .expect("interrupt call");
    assert_eq!(call.function_name, "interrupt");

    // Phase 2: set a short time limit and resume — repr() should timeout
    call.tracker_mut().set_max_duration(Duration::from_millis(10));

    let start = Instant::now();
    let result = call.resume(MontyObject::None, PrintWriter::Stdout);
    let elapsed = start.elapsed();

    let exc = result.unwrap_err();
    assert_eq!(
        exc.exc_type(),
        ExcType::TimeoutError,
        "{label}: expected TimeoutError, got: {exc}"
    );
    let msg = exc.message().unwrap();
    assert!(msg.starts_with("time limit exceeded:"));
    assert!(msg.ends_with("ms > 10ms"));
    assert!(
        elapsed < Duration::from_millis(200),
        "{label}: should terminate promptly, took {elapsed:?}"
    );
}

/// Test that `repr(large_list)` respects the time limit.
///
/// Uses a list of 100K short strings so that repr formatting is slow enough
/// to trigger the timeout.
#[test]
fn timeout_truncation_in_list_repr() {
    let code = r"
x = ['abcdefghij'] * 100_000
interrupt()
repr(x)
";
    assert_repr_timeout(code, "list repr");
}

/// Test that `repr(large_dict)` respects the time limit.
///
/// Uses a dict with 100K entries where values are short strings,
/// making repr formatting slow enough to trigger the timeout.
#[test]
fn timeout_truncation_in_dict_repr() {
    let code = r"
x = {i: 'abcdefghij' for i in range(100_000)}
interrupt()
repr(x)
";
    assert_repr_timeout(code, "dict repr");
}

/// Test that `repr(large_set)` respects the time limit.
///
/// Uses a set of 100K unique strings so that repr formatting is slow enough
/// to trigger the timeout.
#[test]
fn timeout_truncation_in_set_repr() {
    let code = r"
x = {str(i) for i in range(100_000)}
interrupt()
repr(x)
";
    assert_repr_timeout(code, "set repr");
}
/// Test that `re.sub` raises `re.PatternError` when the regex engine hits its backtracking limit.
///
/// The pattern `(a+)+\1b` forces `fancy_regex` into its backtracking VM (due to the
/// backreference `\1`). With enough `a`s followed by a non-matching character, the
/// exponential blowup exceeds the engine's backtracking step limit (~1M steps).
#[test]
fn re_sub_backtracking_limit_raises_pattern_error() {
    let code = r"
import re
re.sub('(a+)+\\1b', 'X', 'a' * 30 + 'c')
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let result = ex.run_no_limits(vec![]);

    assert!(result.is_err(), "backtracking limit should raise an error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::RePatternError);
    assert!(
        exc.message().is_some_and(|m| m.contains("backtrack")),
        "expected backtracking error, got: {exc}"
    );
}

/// Source-driving `itertools` adaptors delegate `next()` to their wrapped
/// iterator on the native Rust stack, so attacker-controlled nesting depth
/// must be charged against the recursion limit — without the guard in
/// `ItertoolsIter::py_next`, deep nesting overflowed the stack and aborted
/// the process instead of raising a recoverable `RecursionError`.
#[test]
fn nested_itertools_adaptors_are_bounded_by_the_recursion_limit() {
    for wrap in [
        "itertools.islice(source, 0, None)",
        "itertools.chain(source)",
        "itertools.pairwise(source)",
        "itertools.compress(source, itertools.repeat(1))",
        "itertools.cycle(source)",
    ] {
        let code = format!(
            r"
import itertools
source = iter([1, 2, 3])
for _ in range(100):
    source = {wrap}
next(source)
"
        );
        let ex = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();

        let limits = ResourceLimits::default().max_recursion_depth(10);
        let result = ex.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout);

        let exc = result.expect_err("nested adaptors should exceed the recursion limit");
        assert_eq!(exc.exc_type(), ExcType::RecursionError, "wrapper: {wrap}");
    }
}

/// Companion to the test above: nesting *below* the limit still works — the
/// per-delegation recursion charge is transient (released as each `next()`
/// returns), so a legal nest must not accumulate depth across iterations.
#[test]
fn nested_itertools_adaptors_below_the_recursion_limit_iterate() {
    let code = r"
import itertools
source = iter([1, 2, 3])
for _ in range(150):
    source = itertools.islice(source, 0, None)
list(source)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();

    let limits = ResourceLimits::default().max_recursion_depth(200);
    let result = ex.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout);

    let list = result.expect("nesting below the recursion limit should succeed");
    assert_eq!(
        list,
        MontyObject::List(vec![MontyObject::Int(1), MontyObject::Int(2), MontyObject::Int(3)])
    );
}

/// Ordering deeply nested namedtuples must raise `RecursionError`, not overflow
/// the native stack. Ordering compares detached item vecs via `cmp_item_seqs`
/// rather than a token-bearing iterator, so it charges its own recursion level;
/// without it, nested namedtuples aborted the process. Covers both the
/// namedtuple-vs-namedtuple and mixed namedtuple-vs-tuple dispatch paths.
#[test]
fn nested_namedtuple_ordering_is_bounded_by_the_recursion_limit() {
    for build in [
        "a = NT(0)\nb = NT(0)\nfor _ in range(100):\n    a = NT(a)\n    b = NT(b)",
        "a = NT(0)\nb = (0,)\nfor _ in range(100):\n    a = NT(a)\n    b = (b,)",
    ] {
        let code = format!(
            r"
from collections import namedtuple
NT = namedtuple('NT', ['x'])
{build}
a < b
"
        );
        let ex = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();

        let limits = ResourceLimits::default().max_recursion_depth(10);
        let result = ex.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout);

        let exc = result.expect_err("nested namedtuple ordering should exceed the recursion limit");
        assert_eq!(exc.exc_type(), ExcType::RecursionError, "build: {build}");
    }
}
