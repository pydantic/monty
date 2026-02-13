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
    // Use multi-character strings to ensure heap allocation (single ASCII chars are interned)
    let code = r"
result = []
for i in range(100, 115):
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
    // Single-digit strings are interned (no allocation), so this uses minimal heap
    let code = r"
result = []
for i in range(9):
    result.append(str(i))
result
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // Allocations: list (1) + range (1) + iterator (1) = 3
    // Note: str(0)...str(8) are single ASCII chars, so they use pre-interned strings
    let limits = ResourceLimits::new().max_allocations(5);
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

    // f-string to create multi-char strings (not interned)
    let code = "foo(1)\nx = []\nfor i in range(10):\n    x.append(f'x{i}')\nlen(x)";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], vec!["foo".to_owned()]).unwrap();

    // First function call should succeed with generous limit
    let limits = ResourceLimits::new().max_allocations(5);
    let (name, args, _kwargs, _call_id, state) = run
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

    // f-string to create multi-char strings (not interned)
    let code = "x = []\nfor i in range(10):\n    x.append(f'x{i}')\nfoo(len(x))\n42";
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
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
fn char_f_string_not_allocated() {
    // Single character f-string interned not not allocated

    let code = "x = []\nfor i in range(10):\n    x.append(f'{i}')";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![], vec!["foo".to_owned()]).unwrap();

    let limits = ResourceLimits::new().max_allocations(4);
    run.run(vec![], LimitedTracker::new(limits), &mut StdPrint).unwrap();
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

    let (name, args, _kwargs, _call_id, state) = run
        .start(vec![], LimitedTracker::new(limits), &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("first call");
    assert_eq!(name, "foo");
    assert_eq!(args, vec![MontyObject::Int(1)]);

    let (name, args, _kwargs, _call_id, state) = state
        .run(MontyObject::None, &mut StdPrint)
        .unwrap()
        .into_function_call()
        .expect("second call");
    assert_eq!(name, "bar");
    assert_eq!(args, vec![MontyObject::Int(2)]);

    let (name, args, _kwargs, _call_id, state) = state
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

// === BigInt large result pre-check tests ===
// These tests verify that operations that would produce very large BigInt results
// are rejected before the computation begins, preventing DoS attacks.

/// Test that large pow operations are rejected by memory limits.
#[test]
fn bigint_pow_memory_limit() {
    // 2 ** 10_000_000 would produce ~1.25MB result
    let code = "2 ** 10000000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // Set a 1MB memory limit - should fail before computing
    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "large pow should exceed memory limit");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that pow with huge exponents is rejected even when the size estimate overflows u64.
///
/// This catches a bug where `estimate_pow_bytes` returned `None` on u64 overflow,
/// and the `if let Some(estimated)` pattern silently skipped the check.
#[test]
fn pow_overflowing_estimate_rejected() {
    // base ~63 bits, exp ~62 bits: estimated result bits = 63 * 3962939411543162624 overflows u64
    let code = "-7234189268083315611 ** 3962939411543162624";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "pow with overflowing estimate should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that pow with a large base and moderate exponent is rejected by memory limits.
///
/// `-7234408281351689115 ** 65327` has a 63-bit base, so the result is ~63*65327 ≈ 4M bits ≈ 514KB.
/// With a 100KB memory limit the pre-check should reject this before computing.
#[test]
fn pow_large_base_moderate_exp_rejected() {
    let code = "-7234408281351689115 ** 65327";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "large pow should exceed memory limit");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that the 4× safety multiplier for pow intermediate allocations catches
/// cases where the final result fits but repeated-squaring intermediates don't.
///
/// `2 ** 500000`: final result = 2 * 500000 bits = 125KB. Without multiplier this
/// passes a 200KB limit. With 4× multiplier: 500KB > 200KB → rejected.
#[test]
fn pow_intermediate_allocation_multiplier() {
    let code = "2 ** 500000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // 200KB limit: final result (125KB) fits, but 4× estimate (500KB) exceeds it
    let limits = ResourceLimits::new().max_memory(200_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(
        result.is_err(),
        "pow should be rejected due to intermediate allocation overhead"
    );
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    // 2 bits * 500000 = 125KB final, × 4 = 500072 bytes (includes base memory offset)
    assert_eq!(
        exc.message(),
        Some("memory limit exceeded: 500072 bytes > 200000 bytes")
    );
}

/// Test that pow still succeeds when the 4× estimate is within the limit.
///
/// `2 ** 100000`: final result = 2 * 100000 bits ≈ 25KB. With 4× multiplier: ~100KB.
/// A 1MB limit should comfortably allow this.
#[test]
fn pow_within_limit_with_multiplier() {
    let code = "x = 2 ** 100000\nx > 0";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_ok(), "pow with 4× estimate under limit should succeed");
    assert_eq!(result.unwrap(), MontyObject::Bool(true));
}

/// Test the exact fuzzer OOM pattern: right-associative chained exponentiation.
///
/// `3 ** 3661666` is the first sub-expression of the fuzzer input
/// `1666**3**366**3**3661666`. Since `**` is right-associative, `3**3661666`
/// is computed first. Base 3 has 2 bits, so: 2 * 3661666 = 7323332 bits ≈ 915KB.
/// With 4× multiplier: 3660KB > 1MB fuzz limit → rejected.
#[test]
fn pow_fuzzer_oom_chained_exponentiation() {
    // This is the subexpression that caused the fuzzer OOM
    let code = "3 ** 3661666";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // 1MB limit (matching the fuzzer's resource limit)
    let limits = ResourceLimits::new().max_memory(1_024 * 1_024);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(
        result.is_err(),
        "fuzzer OOM pattern should be rejected by 4× multiplier"
    );
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    // 2 bits * 3661666 = 915KB final, × 4 = 3661740 bytes
    assert_eq!(
        exc.message(),
        Some("memory limit exceeded: 3661740 bytes > 1048576 bytes")
    );
}

/// Test the full fuzzer input that originally caused OOM.
///
/// The input `1666**3**366**3**3661666` should be rejected before any large
/// intermediate allocation occurs.
#[test]
fn pow_fuzzer_oom_full_input() {
    let code = "1666**3**366**3**3661666";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(1_024 * 1_024);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "full fuzzer OOM input should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    // 3**3661666 is evaluated first (right-associative). Base 3 = 2 bits,
    // so estimate = 2 * 3661666 bits = 915KB. With 4× multiplier: 3661740 bytes > 1MB.
    assert_eq!(
        exc.message(),
        Some("memory limit exceeded: 3661740 bytes > 1048576 bytes")
    );
}

/// Test that large left shift operations are rejected by memory limits.
#[test]
fn bigint_lshift_memory_limit() {
    // 1 << 10_000_000 would produce ~1.25MB result
    let code = "1 << 10000000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // Set a 1MB memory limit - should fail before computing
    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "large lshift should exceed memory limit");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that large multiplication operations are rejected by memory limits.
#[test]
fn bigint_mult_memory_limit() {
    // (2**4_000_000) * (2**4_000_000) would produce ~1MB result
    let code = "big = 2 ** 4000000\nbig * big";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // Set a 1MB memory limit - should fail before computing the multiplication
    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "large mult should exceed memory limit");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that small BigInt operations succeed within memory limits.
#[test]
fn bigint_small_operations_within_limit() {
    // 2 ** 1000 produces ~125 bytes - well under limit
    let code = "x = 2 ** 1000\ny = 1 << 1000\nz = x * 2\nx > 0 and y > 0 and z > 0";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    // Set a 1MB memory limit - should succeed
    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_ok(), "small BigInt operations should succeed within limit");
    let val = result.unwrap();
    assert_eq!(val, MontyObject::Bool(true));
}

/// Test that edge cases (0, 1, -1) with huge exponents succeed even with limits.
/// These produce constant-size results regardless of exponent.
#[test]
fn bigint_edge_cases_always_succeed() {
    // Test each edge case individually to minimize other allocations
    // These edge cases produce constant-size results regardless of exponent:
    // - 0 ** huge = 0
    // - 1 ** huge = 1
    // - (-1) ** huge = 1 or -1
    // - 0 << huge = 0

    // 1MB limit would reject 2**10000000 (~1.25MB) but allows edge cases
    let limits = ResourceLimits::new().max_memory(1_000_000);

    // 0 ** huge = 0
    let code = "0 ** 10000000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run(vec![], LimitedTracker::new(limits.clone()), &mut StdPrint);
    assert!(result.is_ok(), "0 ** huge should succeed");
    assert_eq!(result.unwrap(), MontyObject::Int(0));

    // 1 ** huge = 1
    let code = "1 ** 10000000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run(vec![], LimitedTracker::new(limits.clone()), &mut StdPrint);
    assert!(result.is_ok(), "1 ** huge should succeed");
    assert_eq!(result.unwrap(), MontyObject::Int(1));

    // (-1) ** huge_even = 1
    let code = "(-1) ** 10000000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run(vec![], LimitedTracker::new(limits.clone()), &mut StdPrint);
    assert!(result.is_ok(), "(-1) ** huge_even should succeed");
    assert_eq!(result.unwrap(), MontyObject::Int(1));

    // (-1) ** huge_odd = -1
    let code = "(-1) ** 10000001";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run(vec![], LimitedTracker::new(limits.clone()), &mut StdPrint);
    assert!(result.is_ok(), "(-1) ** huge_odd should succeed");
    assert_eq!(result.unwrap(), MontyObject::Int(-1));

    // 0 << huge = 0
    let code = "0 << 10000000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);
    assert!(result.is_ok(), "0 << huge should succeed");
    assert_eq!(result.unwrap(), MontyObject::Int(0));
}

/// Test that pow() builtin also respects memory limits.
#[test]
fn bigint_builtin_pow_memory_limit() {
    let code = "pow(2, 10000000)";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "builtin pow should respect memory limit");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// Test that large BigInt operations are rejected BEFORE allocation via check_large_result.
///
/// The pre-allocation size check estimates result size and rejects operations that would
/// exceed the memory limit before any memory is actually consumed.
#[test]
fn bigint_rejected_before_allocation() {
    // 2**1000000: base 2 has 2 bits, so estimate = 2 * 1000000 bits = 250KB
    // With 4× safety multiplier for intermediate allocations = 1000KB
    // Set limit to 100KB - the pre-check should reject before allocating
    let code = "2 ** 1000000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "should be rejected before allocation");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert_eq!(
        exc.message(),
        Some("memory limit exceeded: 1000072 bytes > 100000 bytes")
    );
}

// === String/Bytes large result pre-check tests ===
// These tests verify that string/bytes multiplication operations that would produce
// very large results are rejected before the computation begins.

/// Test that large string multiplication is rejected before allocation.
#[test]
fn string_mult_memory_limit() {
    // 'x' * 1000000 = 1MB string
    let code = "'x' * 1000000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "large string mult should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that large bytes multiplication is rejected before allocation.
#[test]
fn bytes_mult_memory_limit() {
    // b'x' * 1000000 = 1MB bytes
    let code = "b'x' * 1000000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "large bytes mult should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that small string multiplication works within limits.
#[test]
fn string_mult_within_limit() {
    // 'abc' * 100 = 300 bytes, well within 100KB limit
    let code = "'abc' * 100 == 'abc' * 100";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_ok(), "small string mult should succeed");
    assert_eq!(result.unwrap(), MontyObject::Bool(true));
}

/// Test that small bytes multiplication works within limits.
#[test]
fn bytes_mult_within_limit() {
    // b'abc' * 100 = 300 bytes, well within 100KB limit
    let code = "b'abc' * 100 == b'abc' * 100";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_ok(), "small bytes mult should succeed");
    assert_eq!(result.unwrap(), MontyObject::Bool(true));
}

/// Test that string multiplication is rejected before allocation via check_large_result.
#[test]
fn string_mult_rejected_before_allocation() {
    // 'x' * 200000 = 200KB string
    // Set limit to 100KB - the pre-check should reject before allocating
    let code = "'x' * 200000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "should be rejected before allocation");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    // The exact size may include some overhead, but should be around 200KB
    assert!(
        exc.message()
            .is_some_and(|m| m.contains("memory limit exceeded") && m.contains("> 100000 bytes")),
        "expected memory limit error with ~200KB size, got: {:?}",
        exc.message()
    );
}

/// Test that large list multiplication is rejected before allocation.
#[test]
fn list_mult_memory_limit() {
    // [1] * 10000 = 10,000 Values = ~160KB (at 16 bytes per Value)
    let code = "[1] * 10000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "large list mult should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that large tuple multiplication is rejected before allocation.
#[test]
fn tuple_mult_memory_limit() {
    // (1,) * 10000 = 10,000 Values = ~160KB (at 16 bytes per Value)
    let code = "(1,) * 10000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "large tuple mult should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that small list multiplication works within limits.
#[test]
fn list_mult_within_limit() {
    // [1, 2, 3] * 20 = 60 Values, well within 100KB limit
    let code = "[1, 2, 3] * 20 == [1, 2, 3] * 20";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_ok(), "small list mult should succeed");
    assert_eq!(result.unwrap(), MontyObject::Bool(true));
}

/// Test that `int * bytes` (int on left) is also rejected by the pre-check.
///
/// This catches a bug where interned bytes/strings bypassed the `mult_sequence`
/// pre-check because `py_mult` handled `InternBytes * Int` inline without
/// checking resource limits.
#[test]
fn int_times_bytes_memory_limit() {
    // int on left side: 1000000 * b'x' = 1MB
    let code = "1000000 * b'x'";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "int * bytes should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that `int * str` (int on left) is also rejected by the pre-check.
#[test]
fn int_times_string_memory_limit() {
    // int on left side: 1000000 * 'x' = 1MB
    let code = "1000000 * 'x'";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "int * str should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that `bigint * bytes` (LongInt on left) is rejected by the pre-check.
#[test]
fn longint_times_bytes_memory_limit() {
    // i64::MAX + 1 = 9223372036854775808, which is a LongInt but fits in usize on 64-bit.
    // Multiplied by 1-byte bytes literal, this would be ~9.2 exabytes.
    let code = "9223372036854775808 * b'x'";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "bigint * bytes should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that `bigint * str` (LongInt on left) is rejected by the pre-check.
#[test]
fn longint_times_string_memory_limit() {
    // i64::MAX + 1 = 9223372036854775808, which is a LongInt but fits in usize on 64-bit.
    let code = "9223372036854775808 * 'x'";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_err(), "bigint * str should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that small tuple multiplication works within limits.
#[test]
fn tuple_mult_within_limit() {
    // (1, 2, 3) * 20 = 60 Values, well within 100KB limit
    let code = "(1, 2, 3) * 20 == (1, 2, 3) * 20";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![], vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), &mut StdPrint);

    assert!(result.is_ok(), "small tuple mult should succeed");
    assert_eq!(result.unwrap(), MontyObject::Bool(true));
}
