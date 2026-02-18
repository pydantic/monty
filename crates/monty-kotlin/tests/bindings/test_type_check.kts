import uniffi.monty_kotlin.*

// === No type errors — should not throw ===
// From test_type_check.py::test_type_check_no_errors
MontyKt.create("x = 1", null, null, null, "")

// === String + int unsupported operator ===
// From test_type_check.py::test_type_check_with_errors
try {
    MontyKt.create(""""hello" + 1""", null, null, null, "")
    throw RuntimeException("Expected TypingException for 'hello' + 1")
} catch (e: MontyException.TypingException) {
    assert(e.message!!.contains("unsupported-operator")) {
        "Expected unsupported-operator in message, got: ${e.message}"
    }
}

// === Function return type mismatch ===
// From test_type_check.py::test_type_check_function_return_type
try {
    MontyKt.create("""
def foo() -> int:
    return "not an int"
""".trimIndent(), null, null, null, "")
    throw RuntimeException("Expected TypingException for wrong return type")
} catch (e: MontyException.TypingException) {
    assert(e.message!!.contains("invalid-return-type")) {
        "Expected invalid-return-type in message, got: ${e.message}"
    }
}

// === Undefined variable ===
// From test_type_check.py::test_type_check_undefined_variable
try {
    MontyKt.create("print(undefined_var)", null, null, null, "")
    throw RuntimeException("Expected TypingException for undefined variable")
} catch (e: MontyException.TypingException) {
    assert(e.message!!.contains("unresolved-reference")) {
        "Expected unresolved-reference in message, got: ${e.message}"
    }
}

// === Valid typed function — should not throw ===
// From test_type_check.py::test_type_check_valid_function
MontyKt.create("""
def add(a: int, b: int) -> int:
    return a + b

add(1, 2)
""".trimIndent(), null, null, null, "")

// === With prefix declaring a variable ===
// From test_type_check.py::test_type_check_with_prefix_code
// Without prefix, x is undefined
try {
    MontyKt.create("result = x + 1", null, null, null, "")
    throw RuntimeException("Expected TypingException for undefined x")
} catch (e: MontyException.TypingException) {
    assert(e.message!!.contains("unresolved-reference")) {
        "Expected unresolved-reference, got: ${e.message}"
    }
}
// With prefix declaring x, it passes
MontyKt.create("result = x + 1", null, null, null, "x = 0")

// === Stubs with external function signature ===
// From test_type_check.py::test_constructor_type_check_stubs_with_external_function
val fetchStubs = "def fetch(url: str) -> str:\n    return ''"
MontyKt.create("""result = fetch("https://example.com")""", null, null, listOf("fetch"), fetchStubs)

// === Stubs with wrong types still catches errors ===
// From test_type_check.py::test_constructor_type_check_stubs_invalid
try {
    MontyKt.create(
        "result: int = x + 1",
        null, null, null,
        """x = "hello""""
    )
    throw RuntimeException("Expected TypingException for str + int")
} catch (e: MontyException.TypingException) {
    assert(e.message!!.contains("unsupported-operator")) {
        "Expected unsupported-operator, got: ${e.message}"
    }
}

// === Line offset test: errors in code with stubs show correct file/line ===
// From test_type_check.py::test_inject_stubs_offset
val typeDefinitions = """
from typing import Any

Messages = list[dict[str, Any]]

async def call_llm(prompt: str, messages: Messages) -> str | Messages:
    ...

prompt: str = ''
""".trimIndent()

val agentCode = """
async def agent(prompt: str, messages: Messages):
    while True:
        print(f'messages so far: {messages}')
        output = await call_llm(prompt, messages)
        if isinstance(output, str):
            return output
        messages.extend(output)

await agent(prompt, [])
""".trimIndent()

// Valid code should pass
MontyKt.create(agentCode, "agent.py", listOf("prompt"), listOf("call_llm"), typeDefinitions)

// Typo in type name should fail with reference to the correct file
try {
    MontyKt.create(
        agentCode.replace("Messages", "MXessages"),
        "agent.py", listOf("prompt"), listOf("call_llm"), typeDefinitions
    )
    throw RuntimeException("Expected TypingException for MXessages typo")
} catch (e: MontyException.TypingException) {
    assert(e.message!!.contains("MXessages")) { "Expected MXessages in error, got: ${e.message}" }
    assert(e.message!!.contains("agent.py")) { "Expected agent.py in error, got: ${e.message}" }
    // The error should point to line 1 of the agent code, not offset by the stubs
    assert(e.message!!.contains("1:")) { "Expected line 1 reference, got: ${e.message}" }
}
