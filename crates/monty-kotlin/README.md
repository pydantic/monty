# monty-kotlin

Kotlin/JVM bindings for the Monty sandboxed Python interpreter via [UniFFI](https://mozilla.github.io/uniffi-rs/).

## Installation

Add the native library and its generated Kotlin bindings to your project. The binding requires [JNA](https://github.com/java-native-access/jna) on the classpath.

```bash
# Build the native library
cargo build -p monty-kotlin --release
```

## Basic Usage

```kotlin
import uniffi.monty_kotlin.*

val noopHandler = object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String = "null"
}

// Simple arithmetic — returns a JSON string
val m = MontyKt.create("1 + 2", null, null, null, null)
val result = m.run("{}", noopHandler, null) // "3"
```

## Input Variables

```kotlin
val m = MontyKt.create("x + y", null, listOf("x", "y"), null, null)
val result = m.run("""{"x": 10, "y": 20}""", noopHandler, null) // "30"
```

Inputs are passed as a JSON object string mapping variable names to JSON-encoded values.
The same `MontyKt` instance can be run repeatedly with different inputs.

## External Functions

Implement `ExternalFunctionHandler` to handle calls from the sandboxed Python code:

```kotlin
val m = MontyKt.create("double(x)", null, null, listOf("double"), null)

val result = m.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
        // argsJson = "[5]"
        val x = argsJson.trim('[', ']').toInt()
        return (x * 2).toString() // return JSON "10"
    }
}, null)
// result == "10"
```

- `argsJson` — positional arguments as a JSON array (e.g. `"[1, \"hello\"]"`)
- `kwargsJson` — keyword arguments as a JSON object (e.g. `"{\"timeout\": 30}"`)
- return value — any JSON string that becomes the Python function's return value

### Typed exceptions from handlers

Since `MontyError` has named fields (not a flat error), Kotlin handlers can throw
typed exceptions that propagate back into the Python sandbox:

```kotlin
val result = m.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
        throw MontyException.RuntimeException(reason = "service unavailable")
    }
}, null)
```

## Concurrent External Functions

When LLM-generated Python code uses `asyncio.gather()` to call multiple external
functions in parallel, use `runConcurrent()` instead of `run()`. It drives the same
execution loop but lets the JVM resolve pending futures as a batch — enabling true
concurrency across IO-bound calls.

```kotlin
import asyncio
results = await asyncio.gather(
    fetch_weather(city="London"),
    fetch_weather(city="Paris"),
    fetch_weather(city="Tokyo"),
)
results
```

```kotlin
val monty = MontyKt.create(code, null, null, listOf("fetch_weather"), null)

val result = monty.runConcurrent("{}", object : ConcurrentFunctionHandler {
    // Called for every external function invocation.
    // Return a JSON string to resolve synchronously, or null to defer.
    override fun call(callId: UInt, functionName: String, argsJson: String, kwargsJson: String): String? =
        null  // defer all calls — they will arrive together in resolvePending

    // Called when Monty is blocked waiting for futures.
    // All deferred calls from one asyncio.gather arrive here at once.
    override fun resolvePending(calls: List<PendingFunctionCall>): List<FunctionCallResult> =
        calls.parallelStream().map { call ->
            val resultJson = weatherService.fetch(call.kwargsJson)  // runs concurrently
            FunctionCallResult(callId = call.callId, resultJson = resultJson)
        }.toList()
}, null)
```

### `run()` vs `runConcurrent()` — which to use

**Use `runConcurrent()` for everything.** It is a strict superset of `run()`: the
execution strategy is determined by what `call()` returns, not by which method you call.

| `call()` returns | Behaviour |
|---|---|
| A JSON string | Resolved immediately — identical to `run()` |
| `null` | Deferred to `resolvePending()` for concurrent resolution |

The practical rule is to base this on your *own* function implementations, not on
the generated Python code:

```kotlin
val ioFunctions = setOf("fetch_weather", "query_db", "call_api")

val handler = object : ConcurrentFunctionHandler {
    override fun call(callId: UInt, functionName: String, argsJson: String, kwargsJson: String): String? =
        if (functionName in ioFunctions) null          // IO-bound → defer
        else resolveSync(functionName, argsJson, kwargsJson)  // fast → resolve now

    override fun resolvePending(calls: List<PendingFunctionCall>): List<FunctionCallResult> =
        calls.parallelStream().map { call ->
            FunctionCallResult(callId = call.callId, resultJson = dispatch(call))
        }.toList()
}
```

When the LLM writes `asyncio.gather(fetch_weather(...), query_db(...))`, the deferred
calls naturally arrive together in `resolvePending()`. When it writes
`result = format_date(x)` (no `await`), `call()` returns the value immediately and
`resolvePending()` is never involved. No code inspection needed.

### Typed exceptions from concurrent handlers

Both `call()` and `resolvePending()` can throw typed exceptions that propagate into
the Python sandbox:

```kotlin
override fun resolvePending(calls: List<PendingFunctionCall>): List<FunctionCallResult> {
    return calls.map { call ->
        try {
            FunctionCallResult(callId = call.callId, resultJson = fetch(call))
        } catch (e: ServiceUnavailableException) {
            throw MontyException.RuntimeException(reason = "service unavailable: ${e.message}")
        }
    }
}
```

## Resource Limits

Constrain execution time, memory, allocations, and recursion depth:

```kotlin
val limits = MontyLimits(
    maxAllocations = null,
    maxDurationSecs = 5.0,       // 5 second wall-clock limit
    maxMemory = 1_000_000uL,     // 1 MB heap limit
    gcInterval = null,
    maxRecursionDepth = 100uL,   // max call stack depth
)

val m = MontyKt.create("fib(30)", null, null, null, null)
val result = m.run("{}", noopHandler, limits)
```

When a limit is exceeded, `run()` throws `MontyException.RuntimeException` with a
message containing `MemoryError`, `TimeoutError`, or `RecursionError`. Pass `null`
for any field to leave that limit disabled, or pass `null` as the entire `limits`
argument for unlimited execution.

## Type Checking

Pass a non-null `typeCheckPrefix` to `create()` to run static type analysis before
execution. Pass `""` to type-check with no stubs, or provide stub declarations:

```kotlin
// Type check with no stubs — catches obvious errors
try {
    MontyKt.create(""""hello" + 1""", null, null, null, "")
} catch (e: MontyException.TypingException) {
    println("Type error: ${e.reason}")
}

// Type check with stub declarations for external functions
val stubs = "def fetch(url: str) -> str: ..."
MontyKt.create("""result = fetch("https://example.com")""", null, null, listOf("fetch"), stubs)
```

Pass `null` for `typeCheckPrefix` to skip type checking entirely (the default).

## MCP Integration

The callback model makes it straightforward to connect Python code to
[MCP](https://modelcontextprotocol.io/) tool servers:

```kotlin
val code = """
result = get_current_time(timezone="UTC")
result
""".trimIndent()

val monty = MontyKt.create(code, null, null, listOf("get_current_time"), null)

val result = monty.run("{}", object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
        // Forward the call to an MCP server and return the JSON-encoded result
        return mcpClient.call(functionName, kwargsJson)
    }
}, null)
```

## Error Handling

```kotlin
try {
    MontyKt.create("def", null, null, null, null)
} catch (e: MontyException.SyntaxException) {
    println("Syntax error: ${e.reason}")
}

try {
    MontyKt.create("1 / 0", null, null, null, null).run("{}", noopHandler, null)
} catch (e: MontyException.RuntimeException) {
    println("Runtime error: ${e.reason}")
}

try {
    MontyKt.create("""from pathlib import Path; Path("/tmp").exists()""", null, null, null, null)
        .run("{}", noopHandler, null)
} catch (e: MontyException.OsCallNotSupported) {
    println("OS call blocked: ${e.reason}")
}
```

## API Reference

### `MontyKt` object

#### `MontyKt.create(code, scriptName, inputs, externalFunctions, typeCheckPrefix)`

Parses and optionally type-checks Python code. Returns an `Arc<MontyKt>` instance
ready for execution.

| Parameter | Type | Description |
|-----------|------|-------------|
| `code` | `String` | Python source code |
| `scriptName` | `String?` | Name for tracebacks (default: `"main.py"`) |
| `inputs` | `List<String>?` | Input variable names |
| `externalFunctions` | `List<String>?` | External function names the code may call |
| `typeCheckPrefix` | `String?` | Stubs for type checking, `""` for no stubs, `null` to skip |

Throws `MontyException.SyntaxException` or `MontyException.TypingException`.

#### `monty.run(inputsJson, handler, limits)`

Executes the code sequentially. Each external function call blocks until the handler
returns a result before execution continues.

| Parameter | Type | Description |
|-----------|------|-------------|
| `inputsJson` | `String` | JSON object of input values, e.g. `"""{"x": 42}"""` |
| `handler` | `ExternalFunctionHandler` | Called for each external function invocation |
| `limits` | `MontyLimits?` | Resource limits, or `null` for unlimited |

Returns a JSON string (the last expression's value). Throws `MontyException.RuntimeException`
or `MontyException.OsCallNotSupported`.

#### `monty.runConcurrent(inputsJson, handler, limits)`

Executes the code with support for concurrent external function resolution. Use this
instead of `run()` when the code may use `asyncio.gather()`. It is a strict superset
of `run()` — the execution strategy is controlled by what `call()` returns.

| Parameter | Type | Description |
|-----------|------|-------------|
| `inputsJson` | `String` | JSON object of input values, e.g. `"""{"x": 42}"""` |
| `handler` | `ConcurrentFunctionHandler` | Dispatches individual calls and resolves batched futures |
| `limits` | `MontyLimits?` | Resource limits, or `null` for unlimited |

Returns a JSON string. Throws `MontyException.RuntimeException` or `MontyException.OsCallNotSupported`.

### `ExternalFunctionHandler` interface

```kotlin
interface ExternalFunctionHandler {
    fun call(functionName: String, argsJson: String, kwargsJson: String): String
}
```

Throw a `MontyException` subclass to propagate an error into the Python sandbox, or
throw any other exception to convert it to a `RuntimeException` via UniFFI's
`UnexpectedUniFFICallbackError` mechanism.

### `ConcurrentFunctionHandler` interface

```kotlin
interface ConcurrentFunctionHandler {
    // Return a JSON string to resolve immediately, or null to defer.
    fun call(callId: UInt, functionName: String, argsJson: String, kwargsJson: String): String?

    // Called when Monty is waiting for deferred futures.
    fun resolvePending(calls: List<PendingFunctionCall>): List<FunctionCallResult>
}
```

### `PendingFunctionCall` data class

| Field | Type | Description |
|-------|------|-------------|
| `callId` | `UInt` | Unique ID — must be echoed back in `FunctionCallResult.callId` |
| `functionName` | `String` | Name of the Python function being called |
| `argsJson` | `String` | Positional arguments as a JSON array |
| `kwargsJson` | `String` | Keyword arguments as a JSON object |

### `FunctionCallResult` data class

| Field | Type | Description |
|-------|------|-------------|
| `callId` | `UInt` | Must match the corresponding `PendingFunctionCall.callId` |
| `resultJson` | `String` | JSON-encoded return value (e.g. `"42"`, `"null"`, `"\"hello\""`) |

### `MontyLimits` data class

| Field | Type | Description |
|-------|------|-------------|
| `maxAllocations` | `ULong?` | Max heap allocations before `MemoryError` |
| `maxDurationSecs` | `Double?` | Max wall-clock seconds before `TimeoutError` |
| `maxMemory` | `ULong?` | Max heap bytes before `MemoryError` |
| `gcInterval` | `ULong?` | Run GC every N allocations |
| `maxRecursionDepth` | `ULong?` | Max call stack depth before `RecursionError` |

### Exception hierarchy

| Kotlin class | Raised when |
|---|---|
| `MontyException.SyntaxException` | Code cannot be parsed |
| `MontyException.TypingException` | Type checking finds errors (requires `typeCheckPrefix`) |
| `MontyException.RuntimeException` | Python runtime error, resource limit exceeded, or handler exception |
| `MontyException.OsCallNotSupported` | Code attempts filesystem, network, or environment access |

All variants expose a `reason: String` field with the error message, and inherit
`message` from `Throwable` (auto-generated as `"reason=<text>"`).

## Testing

```bash
# Build and run all Kotlin binding tests
make test-kotlin

# Or directly
CLASSPATH=jna.jar cargo test -p monty-kotlin
```

Tests require `kotlinc` and `jna.jar` (download from
[Maven Central](https://repo1.maven.org/maven2/net/java/dev/jna/jna/5.13.0/jna-5.13.0.jar)).
