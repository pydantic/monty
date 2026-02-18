import uniffi.monty_kotlin.*

val noopHandler = object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String = "null"
}

// === Normal execution still works when limits are set but not exceeded ===
// From test_limits.py::test_run_with_limits
val mBasic = MontyKt.create("1 + 1", null, null, null, null)
val rBasic = mBasic.run("{}", noopHandler, MontyLimits(
    maxAllocations = null,
    maxDurationSecs = 5.0,
    maxMemory = null,
    gcInterval = null,
    maxRecursionDepth = null,
))
assert(rBasic == "2") { "Expected 2, got $rBasic" }

// === null limits behaves the same as no limits ===
val rNoLimits = mBasic.run("{}", noopHandler, null)
assert(rNoLimits == "2") { "Expected 2, got $rNoLimits" }

// === Limits with inputs still works ===
// From test_limits.py::test_limits_with_inputs
val mInputs = MontyKt.create("x * 2", null, listOf("x"), null, null)
val rInputs = mInputs.run("""{"x": 21}""", noopHandler, MontyLimits(
    maxAllocations = null,
    maxDurationSecs = 5.0,
    maxMemory = null,
    gcInterval = null,
    maxRecursionDepth = null,
))
assert(rInputs == "42") { "Expected 42, got $rInputs" }

// === Recursion limit: exceeded raises RuntimeException (RecursionError) ===
// From test_limits.py::test_recursion_limit
try {
    MontyKt.create("""
def recurse(n):
    if n <= 0:
        return 0
    return 1 + recurse(n - 1)

recurse(10)
""".trimIndent(), null, null, null, null).run("{}", noopHandler, MontyLimits(
        maxAllocations = null,
        maxDurationSecs = null,
        maxMemory = null,
        gcInterval = null,
        maxRecursionDepth = 5uL,
    ))
    throw RuntimeException("Expected RecursionError with maxRecursionDepth=5")
} catch (e: MontyException.RuntimeException) {
    assert(e.reason.contains("RecursionError")) { "Expected RecursionError, got: ${e.reason}" }
}

// === Recursion within limit succeeds ===
// From test_limits.py::test_recursion_limit_ok
val mRecurseOk = MontyKt.create("""
def recurse(n):
    if n <= 0:
        return 0
    return 1 + recurse(n - 1)

recurse(5)
""".trimIndent(), null, null, null, null)
val rRecurseOk = mRecurseOk.run("{}", noopHandler, MontyLimits(
    maxAllocations = null,
    maxDurationSecs = null,
    maxMemory = null,
    gcInterval = null,
    maxRecursionDepth = 100uL,
))
assert(rRecurseOk == "5") { "Expected 5, got $rRecurseOk" }

// === Allocation limit: exceeded raises RuntimeException (MemoryError) ===
// From test_limits.py::test_allocation_limit
try {
    MontyKt.create("""
result = []
for i in range(10000):
    result.append([i])
len(result)
""".trimIndent(), null, null, null, null).run("{}", noopHandler, MontyLimits(
        maxAllocations = 5uL,
        maxDurationSecs = null,
        maxMemory = null,
        gcInterval = null,
        maxRecursionDepth = null,
    ))
    throw RuntimeException("Expected MemoryError with maxAllocations=5")
} catch (e: MontyException.RuntimeException) {
    assert(e.reason.contains("MemoryError")) { "Expected MemoryError, got: ${e.reason}" }
}

// === Memory limit: exceeded raises RuntimeException (MemoryError) ===
// From test_limits.py::test_memory_limit
try {
    MontyKt.create("""
result = []
for i in range(1000):
    result.append('x' * 100)
len(result)
""".trimIndent(), null, null, null, null).run("{}", noopHandler, MontyLimits(
        maxAllocations = null,
        maxDurationSecs = null,
        maxMemory = 100uL,
        gcInterval = null,
        maxRecursionDepth = null,
    ))
    throw RuntimeException("Expected MemoryError with maxMemory=100")
} catch (e: MontyException.RuntimeException) {
    assert(e.reason.contains("MemoryError")) { "Expected MemoryError, got: ${e.reason}" }
}

// === Small operations succeed within memory limits (no exception thrown) ===
// From test_limits.py::test_small_operations_within_limit
// Note: 2**1000 is a BigInt which the JSON bridge serializes as "null"; the test
// verifies it completes without a MemoryError rather than checking the value.
val mSmall = MontyKt.create("2 ** 1000", null, null, null, null)
mSmall.run("{}", noopHandler, MontyLimits(
    maxAllocations = null,
    maxDurationSecs = null,
    maxMemory = 1_000_000uL,
    gcInterval = null,
    maxRecursionDepth = null,
)) // should not throw

// === Timeout: exceeded raises RuntimeException (TimeoutError) ===
// From test_limits.py::test_timeout_enforced_in_builtin_loops
try {
    MontyKt.create("sum(range(10**18))", null, null, null, null).run("{}", noopHandler, MontyLimits(
        maxAllocations = null,
        maxDurationSecs = 0.1,
        maxMemory = null,
        gcInterval = null,
        maxRecursionDepth = null,
    ))
    throw RuntimeException("Expected TimeoutError for infinite sum")
} catch (e: MontyException.RuntimeException) {
    assert(e.reason.contains("TimeoutError")) { "Expected TimeoutError, got: ${e.reason}" }
}

// === Large pow raises MemoryError under memory limit ===
// From test_limits.py::test_pow_memory_limit
try {
    MontyKt.create("2 ** 10000000", null, null, null, null).run("{}", noopHandler, MontyLimits(
        maxAllocations = null,
        maxDurationSecs = null,
        maxMemory = 1_000_000uL,
        gcInterval = null,
        maxRecursionDepth = null,
    ))
    throw RuntimeException("Expected MemoryError for 2**10000000")
} catch (e: MontyException.RuntimeException) {
    assert(e.reason.contains("MemoryError")) { "Expected MemoryError, got: ${e.reason}" }
}

// === Large left shift raises MemoryError under memory limit ===
// From test_limits.py::test_lshift_memory_limit
try {
    MontyKt.create("1 << 10000000", null, null, null, null).run("{}", noopHandler, MontyLimits(
        maxAllocations = null,
        maxDurationSecs = null,
        maxMemory = 1_000_000uL,
        gcInterval = null,
        maxRecursionDepth = null,
    ))
    throw RuntimeException("Expected MemoryError for 1 << 10000000")
} catch (e: MontyException.RuntimeException) {
    assert(e.reason.contains("MemoryError")) { "Expected MemoryError, got: ${e.reason}" }
}
