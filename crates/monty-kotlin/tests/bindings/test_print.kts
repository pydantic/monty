import uniffi.monty_kotlin.*

val noopHandler = object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String = "null"
}

// Note: the Kotlin binding uses PrintWriter::Stdout so actual printed text goes to
// the test runner's stdout and cannot be captured here.  We verify the return value
// of each script (print() returns None → JSON "null").

// === print() returns None ===
// From test_print.py::test_print_returns_none
val mBasic = MontyKt.create("print(\"hello\")", null, null, null, null)
val rBasic = mBasic.run("{}", noopHandler, null)
assert(rBasic == "null") { "Expected null (print returns None), got $rBasic" }

// === Multiple print statements ===
// From test_print.py::test_print_multiple
val mMulti = MontyKt.create("""
print("line 1")
print("line 2")
""".trimIndent(), null, null, null, null)
val rMulti = mMulti.run("{}", noopHandler, null)
assert(rMulti == "null") { "Expected null, got $rMulti" }

// === print with multiple values ===
// From test_print.py::test_print_with_values
val mValues = MontyKt.create("print(1, 2, 3)", null, null, null, null)
val rValues = mValues.run("{}", noopHandler, null)
assert(rValues == "null") { "Expected null, got $rValues" }

// === print with sep ===
// From test_print.py::test_print_with_sep
val mSep = MontyKt.create("print(1, 2, 3, sep=\"-\")", null, null, null, null)
val rSep = mSep.run("{}", noopHandler, null)
assert(rSep == "null") { "Expected null, got $rSep" }

// === print with end ===
// From test_print.py::test_print_with_end
val mEnd = MontyKt.create("print(\"hello\", end=\"!\")", null, null, null, null)
val rEnd = mEnd.run("{}", noopHandler, null)
assert(rEnd == "null") { "Expected null, got $rEnd" }

// === print() with no args ===
// From test_print.py::test_print_empty
val mEmpty = MontyKt.create("print()", null, null, null, null)
val rEmpty = mEmpty.run("{}", noopHandler, null)
assert(rEmpty == "null") { "Expected null, got $rEmpty" }

// === print in loop ===
// From test_print.py::test_print_in_loop
val mLoop = MontyKt.create("""
for i in range(3):
    print(i)
""".trimIndent(), null, null, null, null)
val rLoop = mLoop.run("{}", noopHandler, null)
assert(rLoop == "null") { "Expected null, got $rLoop" }

// === print mixed types ===
// From test_print.py::test_print_mixed_types
val mMixed = MontyKt.create("print(1, \"hello\", True, None)", null, null, null, null)
val rMixed = mMixed.run("{}", noopHandler, null)
assert(rMixed == "null") { "Expected null, got $rMixed" }

// === print with input variable ===
// From test_print.py::test_print_with_inputs
val mInput = MontyKt.create("print(x)", null, listOf("x"), null, null)
val rInput = mInput.run("""{"x": 42}""", noopHandler, null)
assert(rInput == "null") { "Expected null, got $rInput" }

// === print inside a function call ===
// From test_print.py::test_print_callback_raises_in_function (positive case)
val mFunc = MontyKt.create("""
def greet(name):
    print(f"Hello, {name}!")

greet("World")
""".trimIndent(), null, null, null, null)
val rFunc = mFunc.run("{}", noopHandler, null)
assert(rFunc == "null") { "Expected null, got $rFunc" }

// === print result used in expression — print is None, expression still returns ===
// The result of a script ending with a non-print expression should be the expression value.
val mExpr = MontyKt.create("""
print("side effect")
42
""".trimIndent(), null, null, null, null)
val rExpr = mExpr.run("{}", noopHandler, null)
assert(rExpr == "42") { "Expected 42 (last expression), got $rExpr" }

// === map(print, ...) — print used as a callable ===
// From test_print.py::test_map_print
val mMap = MontyKt.create("list(map(print, [1, 2, 3]))", null, null, null, null)
val rMap = mMap.run("{}", noopHandler, null)
// map(print, ...) returns a list of None values → [null, null, null]
assert(rMap == "[null,null,null]") { "Expected [null,null,null], got $rMap" }
