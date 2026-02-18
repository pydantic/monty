import uniffi.monty_kotlin.*

// A no-op handler used for code that doesn't call any external functions.
val noopHandler = object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String = "null"
}

// === Test 1: simple arithmetic ===
val m1 = MontyKt.create("1 + 1", null, null, null, null)
val r1 = m1.run("{}", noopHandler)
assert(r1 == "2") { "Expected 2, got $r1" }

// === Test 2: with inputs ===
val m2 = MontyKt.create("x + y", null, listOf("x", "y"), null, null)
val r2 = m2.run("""{"x": 10, "y": 20}""", noopHandler)
assert(r2 == "30") { "Expected 30, got $r2" }

// === Test 3: string result ===
val m3 = MontyKt.create(""""hello " + name""", null, listOf("name"), null, null)
val r3 = m3.run("""{"name": "world"}""", noopHandler)
assert(r3 == """"hello world"""") { "Expected \"hello world\", got $r3" }

// === Test 4: external function callback ===
val m4 = MontyKt.create("result = double(x=5)\nresult", null, null, listOf("double"), null)
val r4 = m4.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
        // kwargs JSON is {"x": 5} — extract x and double it
        val matchResult = Regex(""""x"\s*:\s*(\d+)""").find(kwargsJson)
        val x = matchResult?.groupValues?.get(1)?.toInt() ?: 0
        return (x * 2).toString()
    }
})
assert(r4 == "10") { "Expected 10, got $r4" }

// === Test 5: runtime error becomes MontyException.RuntimeException ===
// With flat_error, e.message contains the full Display string: "RuntimeError: ..."
try {
    val m5 = MontyKt.create("1 / 0", null, null, null, null)
    m5.run("{}", noopHandler)
    throw RuntimeException("Should have thrown a MontyException.RuntimeException")
} catch (e: MontyException.RuntimeException) {
    assert(e.message!!.contains("ZeroDivisionError")) {
        "Expected ZeroDivisionError in message, got: ${e.message}"
    }
}

// === Test 6: list input and output ===
val m6 = MontyKt.create("items", null, listOf("items"), null, null)
val r6 = m6.run("""{"items": [1, 2, 3]}""", noopHandler)
assert(r6 == "[1,2,3]") { "Expected [1,2,3], got $r6" }

// === Test 7: None return value ===
val m7 = MontyKt.create("x = 1", null, null, null, null)
val r7 = m7.run("{}", noopHandler)
assert(r7 == "null") { "Expected null, got $r7" }

// === Test 8: type checking — valid code passes ===
val m8 = MontyKt.create("x: int = 1\nx + 1", null, null, null, "")
val r8 = m8.run("{}", noopHandler)
assert(r8 == "2") { "Expected 2, got $r8" }

// === Test 9: type checking — type error raises TypingException ===
try {
    MontyKt.create("x: int = 'not an int'", null, null, null, "")
    throw RuntimeException("Should have thrown a MontyException.TypingException")
} catch (e: MontyException.TypingException) {
    // The message contains the type checker's diagnostic output
    assert(e.message!!.isNotEmpty()) {
        "Expected non-empty typing error message, got: ${e.message}"
    }
}

// === Test 10: type checking with stubs prefix ===
val stubs = "def add(a: int, b: int) -> int: ..."
val m10 = MontyKt.create("result = add(1, 2)\nresult", null, null, listOf("add"), stubs)
val r10 = m10.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
        // argsJson = "[1, 2]", return their sum
        val nums = argsJson.trim().removePrefix("[").removeSuffix("]").split(",")
        val sum = nums.sumOf { it.trim().toInt() }
        return sum.toString()
    }
})
assert(r10 == "3") { "Expected 3, got $r10" }
