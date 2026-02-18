import uniffi.monty_kotlin.*

// === Simple await (single async function call) ===
// From test_async.py::test_async
val mSingle = MontyKt.create("await foo()", null, null, listOf("foo"), null)
val rSingle = mSingle.runConcurrent("{}", object : ConcurrentFunctionHandler {
    override fun call(callId: UInt, functionName: String, argsJson: String, kwargsJson: String): String? = null
    override fun resolvePending(calls: List<PendingFunctionCall>): List<FunctionCallResult> =
        calls.map { FunctionCallResult(callId = it.callId, resultJson = "42") }
}, null)
assert(rSingle == "42") { "Expected 42, got $rSingle" }

// === asyncio.gather with 2 concurrent functions ===
// From test_async.py::test_asyncio_gather — all 3 calls arrive in resolvePending at once.
val gatherCode = """
import asyncio
await asyncio.gather(foo(1), bar(2))
""".trimIndent()
val mGather = MontyKt.create(gatherCode, null, null, listOf("foo", "bar"), null)
val rGather = mGather.runConcurrent("{}", object : ConcurrentFunctionHandler {
    override fun call(callId: UInt, functionName: String, argsJson: String, kwargsJson: String): String? = null
    override fun resolvePending(calls: List<PendingFunctionCall>): List<FunctionCallResult> {
        assert(calls.size == 2) { "Expected 2 concurrent calls in gather, got ${calls.size}" }
        return calls.map { call ->
            val n = call.argsJson.removePrefix("[").removeSuffix("]").trim().toInt()
            val result = when (call.functionName) {
                "foo" -> n * 10
                "bar" -> n * 20
                else -> 0
            }
            FunctionCallResult(callId = call.callId, resultJson = result.toString())
        }
    }
}, null)
assert(rGather == "[10,40]") { "Expected [10,40], got $rGather" }

// === asyncio.gather with 3 kwargs-only calls ===
// From test_async.py::test_run_monty_async_multiple_async_functions
val gather3Code = """
import asyncio
results = await asyncio.gather(
    get_data(id=1),
    get_data(id=2),
    get_data(id=3),
)
results
""".trimIndent()
val mGather3 = MontyKt.create(gather3Code, null, null, listOf("get_data"), null)
val rGather3 = mGather3.runConcurrent("{}", object : ConcurrentFunctionHandler {
    override fun call(callId: UInt, functionName: String, argsJson: String, kwargsJson: String): String? = null
    override fun resolvePending(calls: List<PendingFunctionCall>): List<FunctionCallResult> {
        assert(calls.size == 3) { "Expected 3 concurrent calls in gather, got ${calls.size}" }
        return calls.map { call ->
            val id = Regex(""""id"\s*:\s*(\d+)""").find(call.kwargsJson)?.groupValues?.get(1)?.toInt() ?: 0
            FunctionCallResult(callId = call.callId, resultJson = (id * 100).toString())
        }
    }
}, null)
assert(rGather3 == "[100,200,300]") { "Expected [100,200,300], got $rGather3" }

// === Mixed sync and async calls ===
// From test_async.py::test_run_monty_async_mixed_sync_async
// sync_func() resolved immediately; await async_func() deferred to resolvePending.
val mixedCode = """
sync_val = sync_func()
async_val = await async_func()
sync_val + async_val
""".trimIndent()
val mMixed = MontyKt.create(mixedCode, null, null, listOf("sync_func", "async_func"), null)
val rMixed = mMixed.runConcurrent("{}", object : ConcurrentFunctionHandler {
    override fun call(callId: UInt, functionName: String, argsJson: String, kwargsJson: String): String? =
        if (functionName == "sync_func") "10" else null
    override fun resolvePending(calls: List<PendingFunctionCall>): List<FunctionCallResult> =
        calls.map { FunctionCallResult(callId = it.callId, resultJson = "5") }
}, null)
assert(rMixed == "15") { "Expected 15 (10 + 5), got $rMixed" }

// === runConcurrent works for code with no external calls ===
// From test_async.py::test_run_monty_async_no_external_calls
val mBasic = MontyKt.create("1 + 2 + 3", null, null, null, null)
val rBasic = mBasic.runConcurrent("{}", object : ConcurrentFunctionHandler {
    override fun call(callId: UInt, functionName: String, argsJson: String, kwargsJson: String): String? = null
    override fun resolvePending(calls: List<PendingFunctionCall>): List<FunctionCallResult> = emptyList()
}, null)
assert(rBasic == "6") { "Expected 6, got $rBasic" }

// === runConcurrent with inputs (sync resolution) ===
// From test_async.py::test_run_monty_async_with_inputs
// process(x, y) is not awaited, so call() must return the result synchronously.
val mWithInputs = MontyKt.create("process(x, y)", null, listOf("x", "y"), listOf("process"), null)
val rWithInputs = mWithInputs.runConcurrent("""{"x": 6, "y": 7}""", object : ConcurrentFunctionHandler {
    override fun call(callId: UInt, functionName: String, argsJson: String, kwargsJson: String): String? {
        val nums = argsJson.removePrefix("[").removeSuffix("]").split(",").map { it.trim().toInt() }
        return nums.fold(1) { acc, n -> acc * n }.toString()
    }
    override fun resolvePending(calls: List<PendingFunctionCall>): List<FunctionCallResult> = emptyList()
}, null)
assert(rWithInputs == "42") { "Expected 42 (6 * 7), got $rWithInputs" }

// === Error from resolvePending propagates as RuntimeException ===
// From test_async.py::test_run_monty_async_sync_exception
try {
    val mError = MontyKt.create("await fail()", null, null, listOf("fail"), null)
    mError.runConcurrent("{}", object : ConcurrentFunctionHandler {
        override fun call(callId: UInt, functionName: String, argsJson: String, kwargsJson: String): String? = null
        override fun resolvePending(calls: List<PendingFunctionCall>): List<FunctionCallResult> {
            throw MontyException.RuntimeException(reason = "handler failed")
        }
    }, null)
    throw RuntimeException("Expected RuntimeException from resolvePending")
} catch (e: MontyException.RuntimeException) {
    assert(e.reason.isNotEmpty()) { "Expected non-empty error from resolvePending, got: ${e.reason}" }
}
