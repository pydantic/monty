import uniffi.monty_kotlin.*

val noopHandler = object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String = "null"
}

// === Bool roundtrip ===
val mBoolTrue = MontyKt.create("x", null, listOf("x"), null, null)
assert(mBoolTrue.run("""{"x": true}""", noopHandler) == "true") { "Expected true" }

val mBoolFalse = MontyKt.create("x", null, listOf("x"), null, null)
assert(mBoolFalse.run("""{"x": false}""", noopHandler) == "false") { "Expected false" }

val mBoolExpr = MontyKt.create("1 < 2", null, null, null, null)
assert(mBoolExpr.run("{}", noopHandler) == "true") { "Expected true from 1 < 2" }

val mBoolFalseExpr = MontyKt.create("1 > 2", null, null, null, null)
assert(mBoolFalseExpr.run("{}", noopHandler) == "false") { "Expected false from 1 > 2" }

// === Float roundtrip ===
val mFloat = MontyKt.create("x", null, listOf("x"), null, null)
assert(mFloat.run("""{"x": 3.14}""", noopHandler) == "3.14") { "Expected 3.14" }
assert(mFloat.run("""{"x": -2.5}""", noopHandler) == "-2.5") { "Expected -2.5" }
assert(mFloat.run("""{"x": 0.0}""", noopHandler) == "0.0") { "Expected 0.0" }

// === String roundtrip ===
val mStr = MontyKt.create("x", null, listOf("x"), null, null)
assert(mStr.run("""{"x": "hello"}""", noopHandler) == """"hello"""") { "Expected \"hello\"" }
assert(mStr.run("""{"x": ""}""", noopHandler) == """""""") { "Expected empty string" }
assert(mStr.run("""{"x": "unicode: \u00e9\u00e8"}""", noopHandler) == """"unicode: éè"""") { "Expected unicode string" }

// === List roundtrip ===
val mList = MontyKt.create("x", null, listOf("x"), null, null)
assert(mList.run("""{"x": [1, 2, 3]}""", noopHandler) == "[1,2,3]") { "Expected [1,2,3]" }
assert(mList.run("""{"x": []}""", noopHandler) == "[]") { "Expected []" }
assert(mList.run("""{"x": ["a", "b"]}""", noopHandler) == """["a","b"]""") { "Expected [\"a\",\"b\"]" }

// === Tuple output (returned as JSON array) ===
val mTuple = MontyKt.create("(1, 2, 3)", null, null, null, null)
assert(mTuple.run("{}", noopHandler) == "[1,2,3]") { "Expected [1,2,3] for tuple" }

val mEmptyTuple = MontyKt.create("()", null, null, null, null)
assert(mEmptyTuple.run("{}", noopHandler) == "[]") { "Expected [] for empty tuple" }

// === Dict roundtrip ===
val mDict = MontyKt.create("x", null, listOf("x"), null, null)
assert(mDict.run("""{"x": {"a": 1, "b": 2}}""", noopHandler) == """{"a":1,"b":2}""") { "Expected {\"a\":1,\"b\":2}" }
assert(mDict.run("""{"x": {}}""", noopHandler) == "{}") { "Expected {}" }

// Dict output from code
val mDictOut = MontyKt.create("{'a': 1, 'b': 2}", null, null, null, null)
assert(mDictOut.run("{}", noopHandler) == """{"a":1,"b":2}""") { "Expected {\"a\":1,\"b\":2} from code" }

// === Nested list ===
val mNested = MontyKt.create("x", null, listOf("x"), null, null)
assert(mNested.run("""{"x": [[1, 2], [3, 4]]}""", noopHandler) == "[[1,2],[3,4]]") { "Expected [[1,2],[3,4]]" }

// === Nested dict ===
val mNestedDict = MontyKt.create("x", null, listOf("x"), null, null)
assert(mNestedDict.run("""{"x": {"a": {"b": 1}}}""", noopHandler) == """{"a":{"b":1}}""") { "Expected {\"a\":{\"b\":1}}" }

// === Dict input with computation ===
val mDictComp = MontyKt.create("""config["a"] * config["b"]""", null, listOf("config"), null, null)
assert(mDictComp.run("""{"config": {"a": 3, "b": 4}}""", noopHandler) == "12") { "Expected 12" }

// === Multiple runs of same instance ===
val mMulti = MontyKt.create("x * 2", null, listOf("x"), null, null)
assert(mMulti.run("""{"x": 5}""", noopHandler) == "10") { "Expected 10" }
assert(mMulti.run("""{"x": 10}""", noopHandler) == "20") { "Expected 20" }
assert(mMulti.run("""{"x": -3}""", noopHandler) == "-6") { "Expected -6" }

// === Multiline code with assignment ===
val mMultiline = MontyKt.create("""
x = 1
y = 2
x + y
""".trimIndent(), null, null, null, null)
assert(mMultiline.run("{}", noopHandler) == "3") { "Expected 3 from multiline" }

// === Function definition and call ===
val mFunc = MontyKt.create("""
def add(a, b):
    return a + b

add(3, 4)
""".trimIndent(), null, null, null, null)
assert(mFunc.run("{}", noopHandler) == "7") { "Expected 7 from function call" }

// === Function parameter shadows input ===
// From test_inputs.py::test_function_param_shadows_input
val mShadow = MontyKt.create("""
def foo(x):
    return x + 1

foo(x * 2)
""".trimIndent(), null, listOf("x"), null, null)
assert(mShadow.run("""{"x": 5}""", noopHandler) == "11") { "Expected 11 (foo(10) = 11)" }

// Function accessing script input directly (no shadowing)
val mNoShadow = MontyKt.create("""
def double(x):
    return x * 2

result = double(10) + x
result
""".trimIndent(), null, listOf("x"), null, null)
assert(mNoShadow.run("""{"x": 5}""", noopHandler) == "25") { "Expected 25 (double(10)+5)" }
