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

/// `map()` must contextually drop earlier heap results when a later callback
/// raises instead of leaking the native output vector.
#[test]
#[cfg(feature = "ref-count-return")]
fn map_callback_error_drops_prior_heap_results() {
    let code = r"
def build(value):
    if value == 2:
        raise ValueError('stop')
    return [value]

try:
    map(build, [1, 2])
except ValueError:
    pass

result = 'done'
result
";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect("should parse");
    let output = run.run_ref_counts(vec![]).expect("should run");

    assert_eq!(output.unreachable, Vec::<String>::new());
}

/// The two-iterable fast path owns the first argument while advancing the
/// second iterator. If that iterator raises, the first value must be dropped.
#[test]
#[cfg(feature = "ref-count-return")]
fn map_second_iterator_error_drops_current_argument() {
    let code = r"
class RaisingIterator:
    def __iter__(self):
        return self

    def __next__(self):
        raise ValueError('stop')

def combine(a, b):
    return (a, b)

try:
    map(combine, [[1]], RaisingIterator())
except ValueError:
    pass

result = 'done'
result
";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect("should parse");
    let output = run.run_ref_counts(vec![]).expect("should run");

    assert_eq!(output.unreachable, Vec::<String>::new());
}

/// The generic multi-iterable path accumulates arguments in a native vector.
/// A later iterator error must drop every value already collected for the call.
#[test]
#[cfg(feature = "ref-count-return")]
fn map_later_iterator_error_drops_current_arguments() {
    let code = r"
class RaisingIterator:
    def __iter__(self):
        return self

    def __next__(self):
        raise ValueError('stop')

def combine(a, b, c):
    return (a, b, c)

try:
    map(combine, [[1]], [[2]], RaisingIterator())
except ValueError:
    pass

result = 'done'
result
";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect("should parse");
    let output = run.run_ref_counts(vec![]).expect("should run");

    assert_eq!(output.unreachable, Vec::<String>::new());
}

/// The generic multi-iterable path must also drop arguments already collected
/// for a call when a later iterator is exhausted normally.
#[test]
#[cfg(feature = "ref-count-return")]
fn map_later_iterator_exhaustion_drops_current_arguments() {
    let code = r"
def combine(a, b, c):
    return a[0] + b[0] + c[0]

result = map(combine, [[1], [4]], [[2], [5]], [[3]])
result
";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).expect("should parse");
    let output = run.run_ref_counts(vec![]).expect("should run");

    assert_eq!(output.py_object, MontyObject::List(vec![MontyObject::Int(6)]));
    assert_eq!(output.unreachable, Vec::<String>::new());
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
// within builtin functions. Builtins like sum(), sorted(), min(), max() run Rust
// loops entirely within a single bytecode instruction, so they would otherwise
// bypass the VM's dispatch checkpoint entirely. Python iterator advancement and
// the other non-iterator loops therefore poll the tracker themselves, amortized
// via `check_time_every` / `check_memory_time_every`.

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
/// `sum()` iterates via `for_next()`, which polls the time limit every 64th step.
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

/// Calibrate parsing separately so this measures traversal polling, not host speed.
#[test]
fn timeout_in_str_format_field_access_chain() {
    let pause_at_interrupt = |code: &str| {
        let run = MontyRun::new(code.to_owned(), "test.py", vec![], CompileOptions::default()).unwrap();
        let progress = run
            .start(vec![], ResourceTracker::default(), PrintWriter::Stdout)
            .unwrap();
        let call = resolve_name_lookups(progress)
            .unwrap()
            .into_function_call()
            .expect("interrupt call");
        assert_eq!(call.function_name, "interrupt");
        call
    };

    let scan_code = r"
template = '{missing' + '.x' * 1_500_000 + '}'
interrupt()
template.format()
";
    let scan_call = pause_at_interrupt(scan_code);
    let scan_started = Instant::now();
    let scan_result = scan_call.resume(MontyObject::None, PrintWriter::Stdout);
    let scan_elapsed = scan_started.elapsed();
    assert_eq!(scan_result.unwrap_err().exc_type(), ExcType::KeyError);
    let traversal_budget = scan_elapsed.saturating_mul(3);

    let code = r"
class Value:
    pass

value = Value()
value.x = value
template = '{0' + '.x' * 1_500_000 + '}'
interrupt()
template.format(value)
";
    let mut call = pause_at_interrupt(code);

    call.tracker_mut().set_max_duration(traversal_budget);
    let started = Instant::now();
    let result = call.resume(MontyObject::None, PrintWriter::Stdout);
    let elapsed = started.elapsed();

    let exc = result.expect_err("field traversal should exceed the time limit");
    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
    assert!(
        elapsed < traversal_budget.saturating_mul(4),
        "field traversal should terminate promptly, took {elapsed:?}"
    );
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
/// The sort comparison loop polls the time limit (amortized, every 64th comparison).
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
/// The sequence-repetition copy loop in `py_mult` polls the time limit every
/// 64th repetition, so a large multiplication cannot bypass the timeout.
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

/// Test that a dict/set probe restarted by a mutating `__eq__` respects the
/// time limit.
///
/// Every comparison adds another colliding key whose own `__eq__` does the
/// same, so the probe never runs out of new candidates (CPython, walking the
/// live chain, hangs on this too). Re-entering the VM for the callback restarts
/// the dispatch countdown, so only the probe's own `check_time()` can end it.
#[test]
fn timeout_in_mutating_lookup_probe() {
    let template = r"
busy = False


class Mutator:
    def __hash__(self):
        return 1

    def __eq__(self, other):
        global busy
        if not busy:
            busy = True
            ADD_MUTATOR
            busy = False
        return False


container = MAKE_CONTAINER
Mutator() in container
";
    let dict = template
        .replace("ADD_MUTATOR", "container[Mutator()] = 0")
        .replace("MAKE_CONTAINER", "{Mutator(): 0}");
    let set = template
        .replace("ADD_MUTATOR", "container.add(Mutator())")
        .replace("MAKE_CONTAINER", "{Mutator()}");
    assert_timeout_in_builtin(&dict, "dict probe restarted by __eq__");
    assert_timeout_in_builtin(&set, "set probe restarted by __eq__");
}

/// Missing lookups in a fully colliding container must not rescan their
/// already-compared candidates quadratically.
///
/// `H` instances hash constant and are never `eq_is_native`, so a missing
/// probe hands all N entries to the mutation-aware continuation, whose
/// seen-check must be O(1) — a linear scan makes each miss Θ(N²), and that
/// pass reaches no limit poll. Found lookups walk the same candidate chain
/// but never reach the continuation, so timing misses against finds on the
/// same container isolates exactly the continuation's cost — the ratio is
/// independent of machine speed, coverage instrumentation, and feature
/// flags. Measured: healthy ~1.2x, the old linear seen-scan ~4.2x. The 3x
/// threshold therefore sits closer to the regression than to a false alarm —
/// raise it and the test stops catching the bug.
#[test]
fn colliding_lookup_is_not_quadratic() {
    let build_template = r"
class H:
    def __hash__(self):
        return 1


container = MAKE_CONTAINER
for _ in range(800):
    ADD_ENTRY
";
    // 800 found lookups: same per-candidate machinery as the misses below
    // (every comparison still dispatches through the guarded snapshot loop),
    // but the probe ends at its match, before the continuation.
    let found_lookups = r"
for k in list(container):
    assert k in container
";
    // 400 misses, each comparing all 800 candidates and then entering the
    // continuation — in total the same number of comparisons as the finds.
    let missing_lookups = r"
probe = H()
for _ in range(400):
    assert probe not in container
";
    for (label, make_container, add_entry) in [
        ("dict", "{}", "container[H()] = 0"),
        ("set", "set()", "container.add(H())"),
    ] {
        let build = build_template
            .replace("MAKE_CONTAINER", make_container)
            .replace("ADD_ENTRY", add_entry);
        let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
        repl.feed_run(&build, vec![], PrintWriter::Stdout)
            .unwrap_or_else(|e| panic!("{label}: build failed: {e}"));

        let start = Instant::now();
        repl.feed_run(found_lookups, vec![], PrintWriter::Stdout)
            .unwrap_or_else(|e| panic!("{label}: found lookups failed: {e}"));
        let found_elapsed = start.elapsed();

        let start = Instant::now();
        repl.feed_run(missing_lookups, vec![], PrintWriter::Stdout)
            .unwrap_or_else(|e| panic!("{label}: missing lookups failed: {e}"));
        let missing_elapsed = start.elapsed();

        assert!(
            missing_elapsed < found_elapsed * 3,
            "{label}: misses took {missing_elapsed:?} vs finds {found_elapsed:?}, expected linear seen-checks"
        );
    }
}

/// Test that `str.splitlines()` on a large string respects the time limit.
///
/// `str_splitlines()` scans the entire string for line endings in a while loop
/// that polls the limits every 64th line.
#[test]
fn timeout_in_str_splitlines() {
    let code = r"
s = 'a\n' * 5_000_000
s.splitlines()
";
    assert_timeout_in_builtin(code, "str.splitlines()");
}

#[test]
fn timeout_in_str_format_parser() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run("template = '{' + 'x' * 20_000_000", vec![], PrintWriter::Stdout)
        .unwrap();

    let start = Instant::now();
    let exc = repl
        .feed_run("template.format()", vec![], PrintWriter::Stdout)
        .expect_err("an unterminated field must fail without a time limit");
    let full_scan = start.elapsed();
    assert_eq!(exc.exc_type(), ExcType::ValueError);

    repl.tracker_mut().set_max_duration(full_scan / 10);
    let start = Instant::now();
    let exc = repl
        .feed_run("template.format()", vec![], PrintWriter::Stdout)
        .expect_err("the format-string parser must hit the time limit");
    let elapsed = start.elapsed();

    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
    assert!(
        elapsed < full_scan / 2,
        "str.format() should stop during the scan; full scan {full_scan:?}, timed scan {elapsed:?}"
    );
}

#[test]
fn timeout_in_str_format_receiver_snapshot() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run("template = '{missing}' + 'x' * 20_000_000", vec![], PrintWriter::Stdout)
        .unwrap();

    let start = Instant::now();
    let exc = repl
        .feed_run("template.format()", vec![], PrintWriter::Stdout)
        .expect_err("the missing field must fail after snapshotting the receiver");
    let full_snapshot = start.elapsed();
    assert_eq!(exc.exc_type(), ExcType::KeyError);

    repl.tracker_mut().set_max_duration(full_snapshot / 10);
    let start = Instant::now();
    let exc = repl
        .feed_run("template.format()", vec![], PrintWriter::Stdout)
        .expect_err("the receiver snapshot must hit the time limit before field lookup");
    let elapsed = start.elapsed();

    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
    assert!(
        elapsed < full_snapshot / 2,
        "str.format() should stop while copying the receiver; full snapshot {full_snapshot:?}, timed snapshot {elapsed:?}"
    );
}

#[test]
fn timeout_in_str_format_escaped_braces() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run("template = '{{' * 5_000_000", vec![], PrintWriter::Stdout)
        .unwrap();

    let start = Instant::now();
    repl.feed_run("template.format()", vec![], PrintWriter::Stdout).unwrap();
    let full_scan = start.elapsed();

    repl.tracker_mut().set_max_duration(full_scan / 10);
    let start = Instant::now();
    let exc = repl
        .feed_run("template.format()", vec![], PrintWriter::Stdout)
        .expect_err("escaped braces must not bypass the time limit");
    let elapsed = start.elapsed();

    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
    assert!(
        elapsed < full_scan / 2,
        "str.format() should stop during the scan; full scan {full_scan:?}, timed scan {elapsed:?}"
    );
}

#[test]
fn timeout_in_str_format_grouped_padding() {
    let tracker = ResourceTracker::new(ResourceLimits::default().max_duration(Duration::from_millis(10)));
    let mut repl = MontyRepl::new("test.py", tracker, CompileOptions::default());
    let start = Instant::now();
    let exc = repl
        .feed_run("'{:09223372036854775807,}'.format(1)", vec![], PrintWriter::Stdout)
        .expect_err("grouped padding must hit the time limit before allocating the full width");
    let elapsed = start.elapsed();

    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
    assert!(
        elapsed < Duration::from_secs(2),
        "str.format() should terminate promptly, took {elapsed:?}"
    );
}

/// A str field above the large-result threshold is walked in polled steps, so a
/// deadline armed before the call fires inside the format rather than after it.
#[test]
fn timeout_in_str_format_large_str_field() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run("s = 'x' * 20_000_000", vec![], PrintWriter::Stdout)
        .unwrap();

    repl.tracker_mut().set_max_duration(Duration::from_millis(5));
    let start = Instant::now();
    let exc = repl
        .feed_run("'{0:<1}'.format(s)", vec![], PrintWriter::Stdout)
        .expect_err("a large str field must observe the time limit");
    let elapsed = start.elapsed();

    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
    assert!(
        elapsed < Duration::from_secs(2),
        "str.format() should stop inside the format, took {elapsed:?}"
    );
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
// (`repr_sequence_fmt`, `Dict::py_repr_fmt`, `SetInner::repr_fmt`) poll the
// limits every 64th item via `repr_check_time` and write `...[timeout]` when the
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

/// The key pass computes every key before the first comparison, and a key
/// shorter than the dispatch interval reaches no checkpoint — so the key
/// loop's own poll is all that bounds it. `sort`, not `sorted`, so no
/// collection phase polls first.
#[test]
fn timeout_in_sort_key_loop() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "x = [0] * 4_000_000\ndef f(v):\n    return v",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    repl.tracker_mut().set_max_duration(Duration::from_millis(50));
    let start = Instant::now();
    let exc = repl
        .feed_run("x.sort(key=f)", vec![], PrintWriter::Stdout)
        .expect_err("the key loop must hit the time limit");
    let elapsed = start.elapsed();
    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
    // Polled, this stops one budget in at any machine speed; unpolled it runs
    // all 4M key calls before anything re-checks, which takes seconds.
    assert!(
        elapsed < Duration::from_millis(500),
        "should stop promptly, took {elapsed:?}"
    );
}

/// Feeds shorter than the dispatch-checkpoint interval never probe GC inside
/// the run loop, so only the host-boundary probe in `finish_host_turn` keeps
/// a stream of tiny cycle-making snippets from accumulating garbage (and from
/// tripping the boundary memory check on memory a collection would reclaim).
#[test]
#[cfg(feature = "ref-count-return")]
fn short_repl_feeds_still_collect_cycles() {
    let limits = ResourceLimits::default().gc_interval(1);
    let mut repl = MontyRepl::new("test.py", ResourceTracker::new(limits), CompileOptions::default());
    for _ in 0..20 {
        // Rebinding `c` orphans the previous iteration's cycle; each feed is
        // fewer instructions than the dispatch checkpoint interval.
        repl.feed_run("c = []", vec![], PrintWriter::Stdout).unwrap();
        repl.feed_run("c.append(c)", vec![], PrintWriter::Stdout).unwrap();
    }
    assert!(
        repl.heap_entry_count() <= 3,
        "boundary GC probe should collect orphaned cycles: {} live heap entries",
        repl.heap_entry_count()
    );
}

/// `call_function` applies the same host-boundary epilogue as `feed_run`: a
/// call whose over-budget repr truncates (swallowing the timeout) must still
/// fail rather than return the truncated value, and the discarded result's
/// refcounts are released (verified under `memory-model-checks`).
#[test]
fn call_function_rechecks_limits_at_exit() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "x = ['abcdefghij'] * 100_000\ndef f():\n    return repr(x)",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    // Arm a budget only for the call: repr of 100K strings blows it mid-format
    // and truncates, so only the exit re-check can surface the timeout.
    repl.tracker_mut().set_max_duration(Duration::from_millis(10));
    let exc = repl
        .call_function("f", vec![], PrintWriter::Stdout)
        .expect_err("over-budget repr must fail the call even though it truncates");
    assert_eq!(exc.exc_type(), ExcType::TimeoutError);
}

/// The boundary limit check also covers turns ending in a Python exception:
/// session state survives exceptions, so an allocate-then-raise turn must
/// surface the uncatchable resource error, not its own exception, or repeated
/// short erroring feeds could evade the limits entirely.
#[test]
fn erroring_turns_still_hit_limits_at_exit() {
    let mut repl = MontyRepl::new("test.py", ResourceTracker::default(), CompileOptions::default());
    repl.feed_run(
        "x = ['abcdefghij'] * 100_000\ndef f():\n    s = repr(x)\n    raise ValueError(s[:3])",
        vec![],
        PrintWriter::Stdout,
    )
    .unwrap();
    // The over-budget repr truncates (swallowing the timeout), then the raise
    // ends the turn before any dispatch checkpoint can fire.
    repl.tracker_mut().set_max_duration(Duration::from_millis(10));
    let exc = repl
        .call_function("f", vec![], PrintWriter::Stdout)
        .expect_err("the call must fail");
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

/// Test that `repr()` of a widely bound `functools.partial` respects the time
/// limit.
///
/// The bound arguments and keywords are formatted in one native loop, so
/// without the shared `repr_check_time` counter the repr runs to completion
/// before any checkpoint — 500k arguments overshoot the deadline by more than
/// an order of magnitude over the bound asserted here.
#[test]
fn timeout_truncation_in_partial_repr() {
    let code = r"
import functools
def target(*args, **kwargs):
    return 0
p = functools.partial(target, *(['abcdefghij'] * 500_000))
interrupt()
repr(p)
";
    assert_repr_timeout(code, "partial repr");
}

/// Test that `repr(large_set)` respects the time limit.
///
/// The elements are ints rather than strings so that the promptness bound
/// measures the timeout and not the teardown: freeing 300K distinct heap
/// strings after the truncated repr costs more than the whole time budget on
/// a loaded CI machine.
#[test]
fn timeout_truncation_in_set_repr() {
    let code = r"
x = {i for i in range(300_000)}
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

/// Third companion: the level is owed by the delegation, so an adaptor that
/// answers from its own state must not cost one. `accumulate` yields its
/// `initial` without touching its source, so the same nest over it has to fit
/// in the same depth as one over a plain iterator — it needed one more while
/// `ItertoolsIter::py_next` charged before dispatching, and a spent `batched`
/// or a latched `takewhile` in that position was charged the same way.
#[test]
fn itertools_adaptors_charge_recursion_only_when_they_delegate() {
    // The shallowest limit that runs a fixed nest, found rather than pinned:
    // what matters is the difference between the two innermost iterators, not
    // the absolute depth the surrounding frames happen to use.
    let min_depth = |inner: &str| {
        let code = format!(
            r"
import itertools
source = {inner}
for _ in range(20):
    source = itertools.islice(source, 0, None)
next(source)
"
        );
        let ex = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).unwrap();
        (1..=64)
            .find(|&depth| {
                let limits = ResourceLimits::default().max_recursion_depth(depth);
                match ex.run(vec![], ResourceTracker::new(limits), PrintWriter::Stdout) {
                    Ok(value) => {
                        assert_eq!(value, MontyObject::Int(1), "inner: {inner}");
                        true
                    }
                    Err(_) => false,
                }
            })
            .unwrap_or_else(|| panic!("no depth up to 64 ran the nest over {inner}"))
    };

    assert_eq!(
        min_depth("itertools.accumulate([], initial=1)"),
        min_depth("iter([1])"),
        "answering from adaptor state should cost no recursion level"
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

/// Every `itertools` adaptor whose `next` can loop natively without yielding.
///
/// Each pairs a discarding or draining adaptor with an infinite source, so the
/// loop never returns to the VM. `dropwhile` appears twice because a builtin
/// predicate and a short user-defined one fail the same way: the dispatch
/// checkpoint is per-`run()`, so a callback under `CHECK_INTERVAL`
/// instructions restarts the countdown instead of reaching it.
const ITERTOOLS_INFINITE_LOOPS: &[&str] = &[
    "next(itertools.dropwhile(bool, itertools.count(1)))",
    "def always(x):\n    return True\nnext(itertools.dropwhile(always, itertools.count(1)))",
    "next(itertools.filterfalse(bool, itertools.count(1)))",
    "next(itertools.compress(itertools.count(1), itertools.repeat(0)))",
    "next(itertools.islice(itertools.count(1), 10**18, None))",
    "next(itertools.starmap(max, itertools.repeat(itertools.count(1))))",
    "next(itertools.batched(itertools.count(1), 10**18))",
];

/// Test that adaptors discarding items from an infinite source still time out.
///
/// These loops sit inside one bytecode instruction and drive native sources, so
/// nothing returns to the dispatch checkpoint; each must poll the tracker
/// itself or `max_duration` is unenforceable.
#[test]
fn timeout_in_itertools_adaptor_loops() {
    for expr in ITERTOOLS_INFINITE_LOOPS {
        assert_timeout_in_builtin(&format!("import itertools\n{expr}"), expr);
    }
}

/// Test that `a85decode` respects the time limit when `ignorechars` is large.
///
/// Every byte that is no Ascii85 digit reaches `x in ignorechars`, a linear
/// scan for `bytes`, so the decode is quadratic in the two lengths while
/// allocating nothing — a deadline problem no allocation preflight would catch.
/// The lengths are set so that running to completion takes far longer than the
/// promptness bound: without a poll in the loop the builtin returns to the VM's
/// checkpoint only after tens of seconds.
#[test]
fn timeout_in_a85decode_ignorechars() {
    assert_timeout_in_builtin(
        "import base64\ndata = b'\\0' * 1000000\nignore = b'\\xff' * 1000000 + b'\\0'\nbase64.a85decode(data, ignorechars=ignore)",
        "a85decode with large ignorechars",
    );
}
