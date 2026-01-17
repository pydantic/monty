/// Tests for resource limits and garbage collection.
///
/// These tests verify that the `ResourceTracker` system correctly enforces
/// allocation limits, time limits, and triggers garbage collection.
use std::time::Duration;

use monty::{ExcType, LimitedTracker, MontyObject, MontyRun, ResourceLimits, StdPrint};

/// Test that GC properly collects dict cycles via the has_refs() check in allocate().
///
/// This test creates cycles using dict literals and dict setitem. Dict setitem
/// does NOT call mark_potential_cycle(), so the ONLY way may_have_cycles gets
/// set is through the has_refs() check when allocating a dict with refs.
///
/// If has_refs() is disabled, this test will FAIL because GC never runs.
#[test]
#[cfg(feature = "ref-count-return")]
fn gc_collects_dict_cycles_via_has_refs() {
    // Create 200,001 dict cycles. Each iteration:
    // - Creates empty dict d1
    // - Creates dict d2 = {'ref': d1} - d2 is allocated WITH a ref to d1
    //   This triggers has_refs() which sets may_have_cycles = true
    // - Sets d1['ref'] = d2 - creates cycle d1 <-> d2
    //   Dict setitem does NOT call mark_potential_cycle()
    // - On next iteration, both dicts are reassigned, making the cycle unreachable
    //
    // GC runs every 100,000 allocations. With 200,001 iterations:
    // - GC runs at 100k (collects cycles 0-49,999 approximately)
    // - GC runs at 200k (collects more cycles)
    // After GC runs, only the final cycle should remain.
    let code = r"
# Create many dict cycles
for i in range(200001):
    d1 = {}
    d2 = {'ref': d1}  # d2 allocated WITH ref - has_refs() must trigger here
    d1['ref'] = d2    # Cycle formed - dict setitem does NOT call mark_potential_cycle

# Create final result (not a cycle)
result = 'done'
result
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let output = ex.run_ref_counts(vec![]).expect("should succeed");

    // GC_INTERVAL is 100,000. With 200,001 iterations creating dict cycles,
    // GC must have run at least once, resetting allocations_since_gc.
    // If may_have_cycles was never set (has_refs() disabled), GC never runs
    // and allocations_since_gc would be ~400k (2 dicts per iteration).
    assert!(
        output.allocations_since_gc < 100_000,
        "GC should have run (has_refs() must set may_have_cycles): allocations_since_gc = {}",
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

/// Test that GC properly collects self-referencing list cycles.
///
/// This test creates cycles using list.append(), which calls mark_potential_cycle().
/// This tests the mutation-based cycle detection path.
#[test]
#[cfg(feature = "ref-count-return")]
fn gc_collects_list_cycles() {
    // Create 200,001 self-referencing list cycles. Each iteration:
    // - Creates empty list `a`
    // - Appends `a` to itself (creating a self-reference cycle)
    //   This calls mark_potential_cycle() and sets may_have_cycles = true
    // - On next iteration, `a` is reassigned, making the cycle unreachable
    //
    // GC runs every 100,000 allocations. With 200,001 iterations:
    // - GC runs at 100k (collects cycles 0-99,999)
    // - GC runs at 200k (collects cycles 100k-199,999)
    // After GC runs, only the final cycle should remain.
    let code = r"
# Create many self-referencing list cycles
for i in range(200001):
    a = []
    a.append(a)  # Creates cycle via list.append() which calls mark_potential_cycle()

# Create final result (not a cycle)
result = [1, 2, 3]
len(result)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let output = ex.run_ref_counts(vec![]).expect("should succeed");

    // GC_INTERVAL is 100,000. With 200,001 iterations creating list cycles,
    // GC must have run at least twice, resetting allocations_since_gc.
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

/// Test that allocation limits return an error.
#[test]
fn allocation_limit_exceeded() {
    let code = r"
result = []
for i in range(11):
    result.append(str(i))
result
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_allocations(4);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    // Should fail due to allocation limit
    assert!(result.is_err(), "should exceed allocation limit");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("allocation limit exceeded")),
        "expected allocation limit error, got: {exc}"
    );
}

#[test]
fn allocation_limit_not_exceeded() {
    let code = r"
result = []
for i in range(9):
    result.append(str(i))
result
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // Allocations: list (1) + range (1) + iterator (1) + str(0)...str(8) (9) = 12
    let limits = ResourceLimits::new().max_allocations(12);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    // Should succeed
    assert!(result.is_ok(), "should not exceed allocation limit");
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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // Set a short time limit
    let limits = ResourceLimits::new().max_duration(Duration::from_millis(50));
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // Set a generous time limit
    let limits = ResourceLimits::new().max_duration(Duration::from_secs(5));
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    // Should succeed
    assert!(result.is_ok(), "should not exceed time limit");
}

/// Test that memory limits return an error.
#[test]
fn memory_limit_exceeded() {
    // Create code that builds up memory using lists
    // Each iteration creates a new list that gets appended
    let code = r"
result = []
for i in range(100):
    result.append([1, 2, 3, 4, 5])
result
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // Set a very low memory limit (100 bytes) to trigger on nested list allocation
    let limits = ResourceLimits::new().max_memory(100);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    // Should fail due to memory limit
    assert!(result.is_err(), "should exceed memory limit");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

#[test]
fn combined_limits() {
    // Test multiple limits together
    let code = "x = 1 + 2\nx";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new()
        .max_allocations(1000)
        .max_duration(Duration::from_secs(5))
        .max_memory(1024 * 1024);

    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);
    assert!(result.is_ok(), "should succeed with generous limits");
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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // Standard run should succeed
    let result = ex.run_no_limits(vec![]);
    assert!(result.is_ok(), "standard run should succeed");
}

#[test]
fn gc_interval_triggers_collection() {
    // This test verifies that GC can run without crashing
    // We can't easily verify that GC actually collected anything without
    // adding more introspection, but we can verify it runs
    let code = r"
result = []
for i in range(100):
    temp = [1, 2, 3]
    result.append(i)
len(result)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // Set GC to run every 10 allocations
    let limits = ResourceLimits::new().gc_interval(10);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_ok(), "should succeed with GC enabled");
}

#[test]
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
fn executor_iter_resource_limit_on_resume() {
    // Test that resource limits are enforced across function calls
    // First function call succeeds, but resumed execution exceeds limit
    let code = "foo(1)\nx = []\nfor i in range(10):\n    x.append(str(i))\nlen(x)";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], vec!["foo".to_owned()]).unwrap();

    // First function call should succeed with generous limit
    let limits = ResourceLimits::new().max_allocations(5);
    let (name, args, _kwargs, state) = run
        .start(vec![], LimitedTracker::new(limits), &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("function call");
    assert_eq!(name, "foo");
    assert_eq!(args, vec![MontyObject::Int(1)]);

    // Resume - should fail due to allocation limit during the for loop
    let result = state.run(MontyObject::None, &mut StdPrint);
    assert!(result.is_err(), "should exceed allocation limit on resume");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("allocation limit exceeded")),
        "expected allocation limit error, got: {exc}"
    );
}

#[test]
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
fn executor_iter_resource_limit_before_function_call() {
    // Test that resource limits are enforced before first function call
    let code = "x = []\nfor i in range(10):\n    x.append(str(i))\nfoo(len(x))\n42";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], vec!["foo".to_owned()]).unwrap();

    // Should fail before reaching the function call
    let limits = ResourceLimits::new().max_allocations(3);
    let result = run.start(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "should exceed allocation limit before function call");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("allocation limit exceeded")),
        "expected allocation limit error, got: {exc}"
    );
}

#[test]
fn executor_iter_resource_limit_multiple_function_calls() {
    // Test resource limits across multiple function calls
    let code = "foo(1)\nbar(2)\nbaz(3)\n4";
    let run = MontyRun::new(
        code.to_owned(),
        "test.py",
        vec![],
        vec!["foo".to_owned(), "bar".to_owned(), "baz".to_owned()],
    )
    .unwrap();

    // Very tight allocation limit - should still work for simple function calls
    let limits = ResourceLimits::new().max_allocations(100);

    let (name, args, _kwargs, state) = run
        .start(vec![], LimitedTracker::new(limits), &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("first call");
    assert_eq!(name, "foo");
    assert_eq!(args, vec![MontyObject::Int(1)]);

    let (name, args, _kwargs, state) = state
        .run(MontyObject::None, &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("second call");
    assert_eq!(name, "bar");
    assert_eq!(args, vec![MontyObject::Int(2)]);

    let (name, args, _kwargs, state) = state
        .run(MontyObject::None, &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("third call");
    assert_eq!(name, "baz");
    assert_eq!(args, vec![MontyObject::Int(3)]);

    let result = state
        .run(MontyObject::None, &mut StdPrint)
        .unwrap()
        .into_complete()
        .expect("complete");
    assert_eq!(result, MontyObject::Int(4));
}

/// Test that deep recursion triggers memory limit due to namespace tracking.
///
/// Function call namespaces (local variables) are tracked by ResourceTracker.
/// Each recursive call creates a new namespace, which should count against
/// the memory limit.
#[test]
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
fn recursion_respects_memory_limit() {
    // Recursive function that creates stack frames with local variables
    let code = r"
def recurse(n):
    x = 1
    if n > 0:
        return recurse(n - 1)
    return 0
recurse(1000)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // Very tight memory limit - should fail due to namespace memory
    // Each frame needs at least namespace_size * size_of::<Value>() bytes
    let limits = ResourceLimits::new().max_memory(1000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "should exceed memory limit from recursion");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that recursion depth limit returns an error.
#[test]
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
fn recursion_depth_limit_exceeded() {
    let code = r"
def recurse(n):
    if n > 0:
        return recurse(n - 1)
    return 0
recurse(100)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // Set recursion limit to 10
    let limits = ResourceLimits::new().max_recursion_depth(Some(10));
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "should exceed recursion depth limit");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::RecursionError);
    assert!(
        exc.message()
            .is_some_and(|m| m.contains("maximum recursion depth exceeded")),
        "expected recursion depth error, got: {exc}"
    );
}

#[test]
fn recursion_depth_limit_not_exceeded() {
    let code = r"
def recurse(n):
    if n > 0:
        return recurse(n - 1)
    return 0
recurse(5)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // Set recursion limit to 10 - should succeed with 5 levels
    let limits = ResourceLimits::new().max_recursion_depth(Some(10));
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_ok(), "should not exceed recursion depth limit");
}
