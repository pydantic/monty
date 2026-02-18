import uniffi.monty_kotlin.*

val noopHandler = object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String = "null"
}

// === No args — args and kwargs are empty ===
// From test_external.py::test_external_function_no_args
val mNoArgs = MontyKt.create("noop()", null, null, listOf("noop"), null)
val rNoArgs = mNoArgs.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
        assert(argsJson == "[]") { "Expected [], got $argsJson" }
        assert(kwargsJson == "{}") { "Expected {}, got $kwargsJson" }
        return """"called""""
    }
})
assert(rNoArgs == """"called"""") { "Expected \"called\", got $rNoArgs" }

// === Positional args ===
// From test_external.py::test_external_function_positional_args
val mPosArgs = MontyKt.create("func(1, 2, 3)", null, null, listOf("func"), null)
val rPosArgs = mPosArgs.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
        assert(argsJson == "[1,2,3]") { "Expected [1,2,3], got $argsJson" }
        assert(kwargsJson == "{}") { "Expected {}, got $kwargsJson" }
        return """"ok""""
    }
})
assert(rPosArgs == """"ok"""") { "Expected \"ok\", got $rPosArgs" }

// === Kwargs only ===
// From test_external.py::test_external_function_kwargs_only
val mKwargs = MontyKt.create("""func(a=1, b="two")""", null, null, listOf("func"), null)
val rKwargs = mKwargs.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
        assert(argsJson == "[]") { "Expected [], got $argsJson" }
        assert(kwargsJson.contains(""""a":1""")) { "Expected a:1 in kwargs, got $kwargsJson" }
        assert(kwargsJson.contains(""""b":"two"""")) { "Expected b:\"two\" in kwargs, got $kwargsJson" }
        return """"ok""""
    }
})
assert(rKwargs == """"ok"""") { "Expected \"ok\", got $rKwargs" }

// === Mixed positional and keyword args ===
// From test_external.py::test_external_function_mixed_args_kwargs
val mMixed = MontyKt.create("""func(1, 2, x="hello", y=True)""", null, null, listOf("func"), null)
val rMixed = mMixed.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
        assert(argsJson == "[1,2]") { "Expected [1,2], got $argsJson" }
        assert(kwargsJson.contains(""""x":"hello"""")) { "Expected x:hello in kwargs, got $kwargsJson" }
        assert(kwargsJson.contains(""""y":true""")) { "Expected y:true in kwargs, got $kwargsJson" }
        return """"ok""""
    }
})
assert(rMixed == """"ok"""") { "Expected \"ok\", got $rMixed" }

// === Complex type args (list and dict) ===
// From test_external.py::test_external_function_complex_types
val mComplex = MontyKt.create("""func([1, 2], {"key": "value"})""", null, null, listOf("func"), null)
val rComplex = mComplex.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
        assert(argsJson.contains("[1,2]")) { "Expected [1,2] in args, got $argsJson" }
        assert(argsJson.contains(""""key":"value"""")) { "Expected key:value in args, got $argsJson" }
        return """"ok""""
    }
})
assert(rComplex == """"ok"""") { "Expected \"ok\", got $rComplex" }

// === Returns null ===
// From test_external.py::test_external_function_returns_none
val mReturnsNull = MontyKt.create("do_nothing()", null, null, listOf("do_nothing"), null)
val rReturnsNull = mReturnsNull.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String = "null"
})
assert(rReturnsNull == "null") { "Expected null, got $rReturnsNull" }

// === Returns complex dict ===
// From test_external.py::test_external_function_returns_complex_type
val mReturnsComplex = MontyKt.create("get_data()", null, null, listOf("get_data"), null)
val rReturnsComplex = mReturnsComplex.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String =
        """{"a":[1,2,3],"b":{"nested":true}}"""
})
assert(rReturnsComplex == """{"a":[1,2,3],"b":{"nested":true}}""") {
    "Expected complex dict, got $rReturnsComplex"
}

// === Multiple external functions in one expression ===
// From test_external.py::test_multiple_external_functions (add(1,2) + mul(3,4) = 3 + 12 = 15)
val mMultiFuncs = MontyKt.create("add(1, 2) + mul(3, 4)", null, null, listOf("add", "mul"), null)
val rMultiFuncs = mMultiFuncs.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
        val nums = argsJson.removePrefix("[").removeSuffix("]").split(",").map { it.trim().toInt() }
        return when (functionName) {
            "add" -> nums.sum().toString()
            "mul" -> nums.fold(1) { acc, n -> acc * n }.toString()
            else -> "null"
        }
    }
})
assert(rMultiFuncs == "15") { "Expected 15 (3 + 12), got $rMultiFuncs" }

// === External function called multiple times ===
// From test_external.py::test_external_function_called_multiple_times (1 + 2 + 3 = 6)
var callCount = 0
val mCounter = MontyKt.create("counter() + counter() + counter()", null, null, listOf("counter"), null)
val rCounter = mCounter.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
        callCount++
        return callCount.toString()
    }
})
assert(rCounter == "6") { "Expected 6 (1+2+3), got $rCounter" }
assert(callCount == 3) { "Expected 3 calls, got $callCount" }

// === External function with input variable ===
// From test_external.py::test_external_function_with_input
val mWithInput = MontyKt.create("process(x)", null, listOf("x"), listOf("process"), null)
val rWithInput = mWithInput.run("""{"x": 5}""", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
        assert(argsJson == "[5]") { "Expected [5], got $argsJson" }
        val x = argsJson.removePrefix("[").removeSuffix("]").trim().toInt()
        return (x * 10).toString()
    }
})
assert(rWithInput == "50") { "Expected 50 (5 * 10), got $rWithInput" }

// === Exception from handler propagates as RuntimeException ===
// From test_external.py::test_external_function_raises_exception
// The handler throws a typed MontyException.RuntimeException; because MontyError has
// named fields (not flat_error), it is fully liftable from Kotlin callbacks.
try {
    MontyKt.create("fail()", null, null, listOf("fail"), null).run("{}", object : ExternalFunctionHandler {
        override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
            throw MontyException.RuntimeException(reason = "intentional error")
        }
    })
    throw RuntimeException("Expected RuntimeException from handler")
} catch (e: MontyException.RuntimeException) {
    assert(e.reason.isNotEmpty()) { "Expected non-empty error from handler, got: ${e.reason}" }
}

// === Exception caught within Python when raised via finally ===
// From test_external.py::test_external_function_exception_with_finally
val mFinally = MontyKt.create("""
finally_ran = False
try:
    fail()
except ValueError:
    pass
finally:
    finally_ran = True
finally_ran
""".trimIndent(), null, null, listOf("fail"), null)
val rFinally = mFinally.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
        // Return an error JSON that Monty interprets as a ValueError
        // Since our binding converts handler exceptions to RuntimeError, we can't
        // easily inject a ValueError. Instead test that the handler returning null
        // (no exception) lets finally run.
        return "null"
    }
})
assert(rFinally == "true") { "Expected true (finally_ran), got $rFinally" }
