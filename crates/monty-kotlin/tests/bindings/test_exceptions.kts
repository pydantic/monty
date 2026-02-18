import uniffi.monty_kotlin.*

val noopHandler = object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String = "null"
}

// === SyntaxError on create ===
// From test_exceptions.py::test_syntax_error_on_init
try {
    MontyKt.create("def", null, null, null, null)
    throw RuntimeException("Should have thrown SyntaxException")
} catch (e: MontyException.SyntaxException) {
    assert(e.message!!.isNotEmpty()) { "Expected non-empty syntax error" }
}

try {
    MontyKt.create("print(1", null, null, null, null)
    throw RuntimeException("Should have thrown SyntaxException")
} catch (e: MontyException.SyntaxException) {
    assert(e.message!!.isNotEmpty()) { "Expected non-empty syntax error for unclosed paren" }
}

try {
    MontyKt.create("x = = 1", null, null, null, null)
    throw RuntimeException("Should have thrown SyntaxException")
} catch (e: MontyException.SyntaxException) {
    assert(e.message!!.isNotEmpty()) { "Expected non-empty syntax error for invalid syntax" }
}

// === ZeroDivisionError ===
try {
    MontyKt.create("1 / 0", null, null, null, null).run("{}", noopHandler, null)
    throw RuntimeException("Expected ZeroDivisionError")
} catch (e: MontyException.RuntimeException) {
    assert(e.message!!.contains("ZeroDivisionError")) { "Expected ZeroDivisionError, got: ${e.message}" }
}

// === NameError (undefined variable) ===
// From test_exceptions.py::test_name_error
try {
    MontyKt.create("undefined_variable", null, null, null, null).run("{}", noopHandler, null)
    throw RuntimeException("Expected NameError")
} catch (e: MontyException.RuntimeException) {
    assert(e.message!!.contains("NameError")) { "Expected NameError, got: ${e.message}" }
}

// === ValueError raised manually ===
// From test_exceptions.py::test_value_error
try {
    MontyKt.create("raise ValueError('bad value')", null, null, null, null).run("{}", noopHandler, null)
    throw RuntimeException("Expected ValueError")
} catch (e: MontyException.RuntimeException) {
    assert(e.message!!.contains("ValueError")) { "Expected ValueError, got: ${e.message}" }
    assert(e.message!!.contains("bad value")) { "Expected 'bad value', got: ${e.message}" }
}

// === TypeError from string + int ===
// From test_exceptions.py::test_type_error
try {
    MontyKt.create("'string' + 1", null, null, null, null).run("{}", noopHandler, null)
    throw RuntimeException("Expected TypeError")
} catch (e: MontyException.RuntimeException) {
    assert(e.message!!.contains("TypeError")) { "Expected TypeError, got: ${e.message}" }
}

// === IndexError ===
// From test_exceptions.py::test_index_error
try {
    MontyKt.create("[1, 2, 3][10]", null, null, null, null).run("{}", noopHandler, null)
    throw RuntimeException("Expected IndexError")
} catch (e: MontyException.RuntimeException) {
    assert(e.message!!.contains("IndexError")) { "Expected IndexError, got: ${e.message}" }
}

// === KeyError ===
// From test_exceptions.py::test_key_error
try {
    MontyKt.create("{'a': 1}['b']", null, null, null, null).run("{}", noopHandler, null)
    throw RuntimeException("Expected KeyError")
} catch (e: MontyException.RuntimeException) {
    assert(e.message!!.contains("KeyError")) { "Expected KeyError, got: ${e.message}" }
}

// === AssertionError with message ===
// From test_exceptions.py::test_assertion_error_with_message
try {
    MontyKt.create("assert False, 'custom message'", null, null, null, null).run("{}", noopHandler, null)
    throw RuntimeException("Expected AssertionError")
} catch (e: MontyException.RuntimeException) {
    assert(e.message!!.contains("AssertionError")) { "Expected AssertionError, got: ${e.message}" }
    assert(e.message!!.contains("custom message")) { "Expected 'custom message', got: ${e.message}" }
}

// === RuntimeError raised manually ===
// From test_exceptions.py::test_runtime_error
try {
    MontyKt.create("raise RuntimeError('runtime error')", null, null, null, null).run("{}", noopHandler, null)
    throw RuntimeException("Expected RuntimeError")
} catch (e: MontyException.RuntimeException) {
    assert(e.message!!.contains("RuntimeError")) { "Expected RuntimeError in message, got: ${e.message}" }
    assert(e.message!!.contains("runtime error")) { "Expected 'runtime error', got: ${e.message}" }
}

// === NotImplementedError ===
// From test_exceptions.py::test_not_implemented_error
try {
    MontyKt.create("raise NotImplementedError('not implemented')", null, null, null, null).run("{}", noopHandler, null)
    throw RuntimeException("Expected NotImplementedError")
} catch (e: MontyException.RuntimeException) {
    assert(e.message!!.contains("NotImplementedError")) { "Expected NotImplementedError, got: ${e.message}" }
    assert(e.message!!.contains("not implemented")) { "Expected 'not implemented', got: ${e.message}" }
}

// === AttributeError ===
// From test_exceptions.py::test_attribute_error
try {
    MontyKt.create("raise AttributeError('no such attr')", null, null, null, null).run("{}", noopHandler, null)
    throw RuntimeException("Expected AttributeError")
} catch (e: MontyException.RuntimeException) {
    assert(e.message!!.contains("AttributeError")) { "Expected AttributeError, got: ${e.message}" }
    assert(e.message!!.contains("no such attr")) { "Expected 'no such attr', got: ${e.message}" }
}

// === Exception caught within Python code ===
// From test_exceptions.py::test_raise_caught_exception
val mTryCatch = MontyKt.create("""
try:
    1 / 0
except ZeroDivisionError:
    result = 'caught'
result
""".trimIndent(), null, null, null, null)
val rTryCatch = mTryCatch.run("{}", noopHandler, null)
assert(rTryCatch == """"caught"""") { "Expected \"caught\", got $rTryCatch" }

// === Exception in function propagates up ===
// From test_exceptions.py::test_exception_in_function
try {
    MontyKt.create("""
def fail():
    raise ValueError('from function')

fail()
""".trimIndent(), null, null, null, null).run("{}", noopHandler, null)
    throw RuntimeException("Expected ValueError from function")
} catch (e: MontyException.RuntimeException) {
    assert(e.message!!.contains("ValueError")) { "Expected ValueError, got: ${e.message}" }
    assert(e.message!!.contains("from function")) { "Expected 'from function', got: ${e.message}" }
}

// === Missing input raises RuntimeException ===
// From test_inputs.py::test_missing_input_raises
try {
    val m = MontyKt.create("x + y", null, listOf("x", "y"), null, null)
    m.run("""{"x": 1}""", noopHandler, null)
    throw RuntimeException("Expected RuntimeException for missing input")
} catch (e: MontyException.RuntimeException) {
    assert(e.message!!.contains("Missing required input")) {
        "Expected 'Missing required input', got: ${e.message}"
    }
}
