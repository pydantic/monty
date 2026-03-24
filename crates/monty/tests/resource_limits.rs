/// Tests for resource limits and garbage collection.
///
/// These tests verify that the `ResourceTracker` system correctly enforces
/// allocation limits, time limits, and triggers garbage collection.
use std::time::{Duration, Instant};

use monty::{
    ExcType, LimitedTracker, MontyObject, MontyRun, NameLookupResult, PrintWriter, ResourceLimits, RunProgress,
};

/// Resolves consecutive `NameLookup` yields by providing a `Function` object for each name.
///
/// External functions are no longer declared upfront. Instead, the VM yields `NameLookup`
/// when it encounters an unresolved name. This helper resolves all such lookups until
/// a different progress variant is reached.
fn resolve_name_lookups<T: monty::ResourceTracker>(
    mut progress: RunProgress<T>,
) -> Result<RunProgress<T>, monty::MontyException> {
    while let RunProgress::NameLookup(lookup) = progress {
        let name = lookup.name.clone();
        progress = lookup.resume(
            NameLookupResult::Value(MontyObject::Function { name, docstring: None }),
            PrintWriter::Stdout,
        )?;
    }
    Ok(progress)
}

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_allocations(4);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Allocations: list (1) + range (1) + iterator (1) = 3
    // Note: str(0)...str(8) are single ASCII chars, so they use pre-interned strings
    let limits = ResourceLimits::new().max_allocations(5);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Set a short time limit
    let limits = ResourceLimits::new().max_duration(Duration::from_millis(50));
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Set a generous time limit
    let limits = ResourceLimits::new().max_duration(Duration::from_secs(5));
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Set a very low memory limit (100 bytes) to trigger on nested list allocation
    let limits = ResourceLimits::new().max_memory(100);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new()
        .max_allocations(1000)
        .max_duration(Duration::from_secs(5))
        .max_memory(1024 * 1024);

    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);
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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Set GC to run every 10 allocations
    let limits = ResourceLimits::new().gc_interval(10);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let run = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // First function call should succeed with generous limit
    let limits = ResourceLimits::new().max_allocations(5);
    let progress = run
        .start(vec![], LimitedTracker::new(limits), PrintWriter::Stdout)
        .unwrap();
    let call = resolve_name_lookups(progress)
        .unwrap()
        .into_function_call()
        .expect("function call");
    assert_eq!(call.function_name, "foo");
    assert_eq!(call.args, vec![MontyObject::Int(1)]);

    // Resume - should fail due to allocation limit during the for loop
    let result = call.resume(MontyObject::None, PrintWriter::Stdout);
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
    let run = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Should fail before reaching the function call
    let limits = ResourceLimits::new().max_allocations(3);
    let result = run.start(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let run = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_allocations(4);
    run.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout)
        .unwrap();
}

#[test]
fn executor_iter_resource_limit_multiple_function_calls() {
    // Test resource limits across multiple function calls
    let code = "foo(1)\nbar(2)\nbaz(3)\n4";
    let run = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Very tight allocation limit - should still work for simple function calls
    let limits = ResourceLimits::new().max_allocations(100);

    let progress = run
        .start(vec![], LimitedTracker::new(limits), PrintWriter::Stdout)
        .unwrap();
    let call = resolve_name_lookups(progress)
        .unwrap()
        .into_function_call()
        .expect("first call");
    assert_eq!(call.function_name, "foo");
    assert_eq!(call.args, vec![MontyObject::Int(1)]);

    let progress = call.resume(MontyObject::None, PrintWriter::Stdout).unwrap();
    let call = resolve_name_lookups(progress)
        .unwrap()
        .into_function_call()
        .expect("second call");
    assert_eq!(call.function_name, "bar");
    assert_eq!(call.args, vec![MontyObject::Int(2)]);

    let progress = call.resume(MontyObject::None, PrintWriter::Stdout).unwrap();
    let call = resolve_name_lookups(progress)
        .unwrap()
        .into_function_call()
        .expect("third call");
    assert_eq!(call.function_name, "baz");
    assert_eq!(call.args, vec![MontyObject::Int(3)]);

    let result = call
        .resume(MontyObject::None, PrintWriter::Stdout)
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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Very tight memory limit - should fail due to namespace memory
    // Each frame needs at least namespace_size * size_of::<Value>() bytes
    let limits = ResourceLimits::new().max_memory(1000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Set recursion limit to 10
    let limits = ResourceLimits::new().max_recursion_depth(Some(10));
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Set recursion limit to 10 - should succeed with 5 levels
    let limits = ResourceLimits::new().max_recursion_depth(Some(10));
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Set a 1MB memory limit - should fail before computing
    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // 200KB limit: final result (125KB) fits, but 4× estimate (500KB) exceeds it
    let limits = ResourceLimits::new().max_memory(200_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // 1MB limit (matching the fuzzer's resource limit)
    let limits = ResourceLimits::new().max_memory(1_024 * 1_024);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(1_024 * 1_024);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Set a 1MB memory limit - should fail before computing
    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Set a 1MB memory limit - should fail before computing the multiplication
    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Set a 1MB memory limit - should succeed
    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run(vec![], LimitedTracker::new(limits.clone()), PrintWriter::Stdout);
    assert!(result.is_ok(), "0 ** huge should succeed");
    assert_eq!(result.unwrap(), MontyObject::Int(0));

    // 1 ** huge = 1
    let code = "1 ** 10000000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run(vec![], LimitedTracker::new(limits.clone()), PrintWriter::Stdout);
    assert!(result.is_ok(), "1 ** huge should succeed");
    assert_eq!(result.unwrap(), MontyObject::Int(1));

    // (-1) ** huge_even = 1
    let code = "(-1) ** 10000000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run(vec![], LimitedTracker::new(limits.clone()), PrintWriter::Stdout);
    assert!(result.is_ok(), "(-1) ** huge_even should succeed");
    assert_eq!(result.unwrap(), MontyObject::Int(1));

    // (-1) ** huge_odd = -1
    let code = "(-1) ** 10000001";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run(vec![], LimitedTracker::new(limits.clone()), PrintWriter::Stdout);
    assert!(result.is_ok(), "(-1) ** huge_odd should succeed");
    assert_eq!(result.unwrap(), MontyObject::Int(-1));

    // 0 << huge = 0
    let code = "0 << 10000000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);
    assert!(result.is_ok(), "0 << huge should succeed");
    assert_eq!(result.unwrap(), MontyObject::Int(0));
}

/// Test that pow() builtin also respects memory limits.
#[test]
fn bigint_builtin_pow_memory_limit() {
    let code = "pow(2, 10000000)";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_ok(), "small string mult should succeed");
    assert_eq!(result.unwrap(), MontyObject::Bool(true));
}

/// Test that small bytes multiplication works within limits.
#[test]
fn bytes_mult_within_limit() {
    // b'abc' * 100 = 300 bytes, well within 100KB limit
    let code = "b'abc' * 100 == b'abc' * 100";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_ok(), "small bytes mult should succeed");
    assert_eq!(result.unwrap(), MontyObject::Bool(true));
}

/// Test that string multiplication is rejected before allocation via check_large_result.
#[test]
fn string_mult_rejected_before_allocation() {
    // 'x' * 200000 = 200KB string
    // Set limit to 100KB - the pre-check should reject before allocating
    let code = "'x' * 200000";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000); // 100KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_ok(), "small tuple mult should succeed");
    assert_eq!(result.unwrap(), MontyObject::Bool(true));
}

// === Timeout enforcement in builtin iteration loops ===
// These tests verify that `max_duration_secs` is enforced inside Rust-side loops
// within builtin functions. Previously, builtins like sum(), sorted(), min(), max()
// ran Rust loops entirely within a single bytecode instruction, bypassing the VM's
// per-instruction timeout check. The fix adds `heap.check_time()` calls inside
// `MontyIter::for_next()` and other non-iterator loops.

/// Helper: runs code with a short time limit and asserts it produces a TimeoutError promptly.
fn assert_timeout_in_builtin(code: &str, label: &str) {
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_duration(Duration::from_millis(100));
    let start = std::time::Instant::now();
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);
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
/// The `list()` constructor collects via `MontyIter::collect()` -> `for_next()`.
#[test]
fn timeout_in_list_constructor() {
    assert_timeout_in_builtin("list(range(10**18))", "list(range(10**18))");
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
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
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
/// The `tuple()` constructor collects via `MontyIter::collect()` -> `for_next()`.
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
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
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
/// The `mult_sequence()` copy loop now calls `heap.check_time()` on each
/// repetition to prevent large sequence multiplications from bypassing timeout.
#[test]
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
fn timeout_in_list_repetition() {
    assert_timeout_in_builtin("[1, 2, 3] * 10_000_000", "list repetition");
}

/// Test that `(1,) * 10_000_000` (tuple repetition) respects the time limit.
///
/// Same as list repetition but for tuples — both paths in `mult_sequence()`
/// now check the time limit.
#[test]
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
fn timeout_in_tuple_repetition() {
    assert_timeout_in_builtin("(1, 2, 3) * 10_000_000", "tuple repetition");
}

/// Test that comparing two large equal lists respects the time limit.
///
/// `List::py_eq()` iterates element-wise comparing pairs. With large equal lists,
/// it must compare every element before returning True.
#[test]
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
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
/// `Dict::py_eq()` iterates all entries checking keys and values. With large equal
/// dicts, it must check every entry before returning True.
#[test]
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
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
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
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
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
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

/// Helper: builds a large object without time limit, then runs `repr()` on it
/// with a short time limit and asserts it produces a TimeoutError promptly.
///
/// The code must call `interrupt()` between object construction and `repr()`.
fn assert_repr_timeout(code: &str, label: &str) {
    let run = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // Phase 1: build the large object with no time limit
    let limits = ResourceLimits::new();
    let progress = run
        .start(vec![], LimitedTracker::new(limits), PrintWriter::Stdout)
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
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
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
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
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
#[cfg_attr(
    feature = "ref-count-panic",
    ignore = "resource exhaustion doesn't guarantee heap state consistency"
)]
fn timeout_truncation_in_set_repr() {
    let code = r"
x = {str(i) for i in range(100_000)}
interrupt()
repr(x)
";
    assert_repr_timeout(code, "set repr");
}

/// Test that `str.replace` with amplification is rejected before allocation.
///
/// `'a' * 1000` is 1KB (within limit), but replacing each 'a' with a 1KB string
/// produces a 1MB result. The pre-check should reject this before `String::replace()`
/// allocates the result on the Rust heap.
#[test]
fn str_replace_amplification_memory_limit() {
    let code = r"
s = 'a' * 1000
s.replace('a', 'b' * 1000)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(500_000); // 500KB limit
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "str.replace amplification should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that small `str.replace` works within limits.
#[test]
fn str_replace_within_limit() {
    let code = "'hello world'.replace('world', 'rust') == 'hello rust'";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_ok(), "small str.replace should succeed");
    assert_eq!(result.unwrap(), MontyObject::Bool(true));
}

/// Test that `bytes.replace` with amplification is rejected before allocation.
#[test]
fn bytes_replace_amplification_memory_limit() {
    let code = r"
s = b'a' * 1000
s.replace(b'a', b'b' * 1000)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(500_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "bytes.replace amplification should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that `str.replace` with empty pattern amplification is rejected.
///
/// Empty pattern inserts `new` before each char and after the last, so
/// result size = input_len * (new_len + 1).
#[test]
fn str_replace_empty_pattern_memory_limit() {
    let code = r"
s = 'a' * 500
s.replace('', 'x' * 1000)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(200_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(
        result.is_err(),
        "str.replace with empty pattern amplification should be rejected"
    );
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// Test that `str.ljust` with huge width is rejected before allocation.
///
/// Without the pre-check, `String::with_capacity(width)` would allocate
/// directly on the Rust heap, bypassing the memory tracker entirely.
#[test]
fn str_ljust_memory_limit() {
    let code = "'x'.ljust(2000000)";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "str.ljust with huge width should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that `str.rjust` with huge width is rejected before allocation.
#[test]
fn str_rjust_memory_limit() {
    let code = "'x'.rjust(2000000)";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "str.rjust with huge width should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// Test that `str.center` with huge width is rejected before allocation.
#[test]
fn str_center_memory_limit() {
    let code = "'x'.center(2000000)";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "str.center with huge width should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// Test that `str.zfill` with huge width is rejected before allocation.
#[test]
fn str_zfill_memory_limit() {
    let code = "'42'.zfill(2000000)";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "str.zfill with huge width should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// Test that small padding operations work within limits.
#[test]
fn str_padding_within_limit() {
    let code = "'hi'.ljust(10) == 'hi        '";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_ok(), "small padding should succeed");
    assert_eq!(result.unwrap(), MontyObject::Bool(true));
}

/// Test that `bytes.ljust` with huge width is rejected before allocation.
#[test]
fn bytes_ljust_memory_limit() {
    let code = "b'x'.ljust(2000000)";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "bytes.ljust with huge width should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// Test that `bytes.rjust` with huge width is rejected before allocation.
#[test]
fn bytes_rjust_memory_limit() {
    let code = "b'x'.rjust(2000000)";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "bytes.rjust with huge width should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// Test that `bytes.center` with huge width is rejected before allocation.
#[test]
fn bytes_center_memory_limit() {
    let code = "b'x'.center(2000000)";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "bytes.center with huge width should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// Test that `bytes.zfill` with huge width is rejected before allocation.
#[test]
fn bytes_zfill_memory_limit() {
    let code = "b'42'.zfill(2000000)";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "bytes.zfill with huge width should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// Test that f-string formatting with huge width is rejected before allocation.
#[test]
fn fstring_dynamic_width_memory_limit() {
    // Dynamic format spec via f-string nesting: {w} produces a runtime-parsed spec
    let code = "w = 2000000\nf\"{'x':>{w}}\"";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "f-string with huge dynamic width should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

// === re.sub() memory tracking tests ===
// These tests verify that the single-pass replacement loop in `re.sub()` tracks
// the running output size and bails out when the resource limit is exceeded.

/// Test that `re.sub` with every-char pattern amplification is rejected.
///
/// Pattern 'a' matches every character in 'aaa...'. Each replacement expands
/// 1 byte → 1000 bytes, so the output grows to ~1MB which exceeds the 500KB limit.
/// The inline loop catches this after a few hundred matches.
#[test]
fn re_sub_amplification_memory_limit() {
    let code = r"
import re
s = 'a' * 1000
re.sub('a', 'b' * 1000, s)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(500_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "re.sub amplification should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
    assert!(
        exc.message().is_some_and(|m| m.contains("memory limit exceeded")),
        "expected memory limit error, got: {exc}"
    );
}

/// Test that `re.sub` with empty pattern amplification is rejected.
///
/// Empty pattern matches N+1 times for N-char input (between and around every
/// character). Each match inserts 1000 bytes, so 501 matches × 1000 ≈ 500KB
/// which exceeds the 200KB limit.
#[test]
fn re_sub_empty_pattern_amplification_memory_limit() {
    let code = r"
import re
s = 'a' * 500
re.sub('', 'x' * 1000, s)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(200_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(
        result.is_err(),
        "re.sub with empty pattern amplification should be rejected"
    );
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// Test that `pattern.sub` (compiled pattern method) is also rejected.
#[test]
fn re_pattern_sub_amplification_memory_limit() {
    let code = r"
import re
p = re.compile('a')
s = 'a' * 1000
p.sub('b' * 1000, s)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(500_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "pattern.sub amplification should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
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
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(500_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "backtracking limit should raise an error");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::RePatternError);
    assert!(
        exc.message().is_some_and(|m| m.contains("backtrack")),
        "expected backtracking error, got: {exc}"
    );
}

// --- Selective patterns: few matches in large text stay within limits ---

/// Test that a selective pattern on large text passes.
///
/// The pattern `xxx` only matches 3 times (at positions 0, 3, 6 in the 9-char prefix),
/// so the result is ~10000 - 9 + 300 = 10291 bytes — well within the 500KB limit.
#[test]
fn re_sub_selective_pattern_passes() {
    // 'xxx' repeated 3 times at the start, rest is 'a's
    let code = r"
import re
s = 'xxx' * 3 + 'a' * 9991
result = re.sub('xxx', 'y' * 100, s)
len(result)  == 9991 + 3 * 100
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(500_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(
        result.is_ok(),
        "selective pattern with few matches should pass: {result:?}"
    );
    assert_eq!(result.unwrap(), MontyObject::Bool(true));
}

/// Test that a digit-matching pattern on mostly-text input passes.
///
/// Pattern `\d+` matches only the 10-digit number, so the result is
/// 990 + 200 = 1190 bytes — well within the 150KB limit.
#[test]
fn re_sub_digit_pattern_passes() {
    let code = r"
import re
s = 'a' * 990 + '1234567890'
result = re.sub('\d+', 'X' * 200, s)
len(result) == 990 + 200
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(150_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_ok(), "digit pattern on mostly-text should pass: {result:?}");
    assert_eq!(result.unwrap(), MontyObject::Bool(true));
}

/// Test that every-char amplification is still rejected even with a generic pattern.
///
/// Pattern `.` matches every character (10000 matches), each expanding 1 → 1000 bytes.
/// The inline loop catches this after a few hundred matches once the running output
/// size exceeds the 500KB limit.
#[test]
fn re_sub_every_char_amplification_rejected() {
    let code = r"
import re
s = 'a' * 10000
re.sub('.', 'b' * 1000, s)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(500_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "every-char pattern amplification should be rejected");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

// --- General re.sub tests ---

/// Test that small `re.sub` works within limits.
#[test]
fn re_sub_within_limit() {
    let code = r"
import re
re.sub('world', 'rust', 'hello world') == 'hello rust'
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_ok(), "small re.sub should succeed");
    assert_eq!(result.unwrap(), MontyObject::Bool(true));
}

/// Test that `re.sub` with count parameter limits replacements correctly.
///
/// `count=5` caps replacements to 5, so the result is
/// 995 unchanged bytes + 5 × 100 replacement bytes = 1495 bytes.
#[test]
fn re_sub_with_count_within_limit() {
    let code = r"
import re
re.sub('a', 'b' * 100, 'a' * 1000, count=5) == 'b' * 500 + 'a' * 995
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(500_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_ok(), "re.sub with small count should succeed");
    assert_eq!(result.unwrap(), MontyObject::Bool(true));
}

// === Container growth memory tracking tests ===
// These tests verify that mutable container operations (append, insert, extend, iadd,
// dict setitem, set add) correctly track memory growth against configured limits.

/// `list.append()` in a loop must respect memory limits.
#[test]
fn list_append_respects_memory_limit() {
    let code = r"
x = []
for i in range(1000000):
    x.append(i)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(10_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "should exceed memory limit via list.append");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// `list.insert()` in a loop must respect memory limits.
#[test]
fn list_insert_respects_memory_limit() {
    let code = r"
x = []
for i in range(1000000):
    x.insert(0, i)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(10_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "should exceed memory limit via list.insert");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// `list.extend()` must respect memory limits.
#[test]
fn list_extend_respects_memory_limit() {
    let code = r"
x = []
x.extend(range(1000000))
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(10_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "should exceed memory limit via list.extend");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// `list += list` (iadd with self-doubling) must respect memory limits.
#[test]
fn list_iadd_respects_memory_limit() {
    let code = r"
x = list(range(100))
for i in range(20):
    x += x
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(100_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "should exceed memory limit via list iadd");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// `dict[k] = v` in a loop must respect memory limits.
#[test]
fn dict_setitem_respects_memory_limit() {
    let code = r"
x = {}
for i in range(1000000):
    x[i] = i
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(10_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "should exceed memory limit via dict setitem");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// `set.add()` in a loop must respect memory limits.
#[test]
fn set_add_respects_memory_limit() {
    let code = r"
x = set()
for i in range(1000000):
    x.add(i)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(10_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "should exceed memory limit via set.add");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// List comprehension must respect memory limits.
#[test]
fn list_comprehension_respects_memory_limit() {
    let code = r"
x = [i for i in range(1000000)]
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    let limits = ResourceLimits::new().max_memory(10_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(result.is_err(), "should exceed memory limit via list comprehension");
    let exc = result.unwrap_err();
    assert_eq!(exc.exc_type(), ExcType::MemoryError);
}

/// Moderate container operations within generous limits should still succeed.
#[test]
fn moderate_container_growth_within_limits() {
    let code = r"
x = []
for i in range(100):
    x.append(i)

d = {}
for i in range(100):
    d[i] = i

s = set()
for i in range(100):
    s.add(i)

len(x) + len(d) + len(s)
";
    let ex = MontyRun::new(code.to_owned(), "test.py", vec![]).unwrap();

    // 1MB limit — plenty of room for 300 total elements
    let limits = ResourceLimits::new().max_memory(1_000_000);
    let result = ex.run(vec![], LimitedTracker::new(limits), PrintWriter::Stdout);

    assert!(
        result.is_ok(),
        "moderate operations should succeed within generous limit"
    );
    assert_eq!(result.unwrap(), MontyObject::Int(300));
}
