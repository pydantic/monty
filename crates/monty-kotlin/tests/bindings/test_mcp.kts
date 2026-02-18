import uniffi.monty_kotlin.*

// Attempt to start the MCP time server. Skip gracefully if uvx is not available.
val process = try {
    ProcessBuilder("uvx", "mcp-server-time")
        .redirectErrorStream(false)
        .start()
} catch (e: Exception) {
    println("SKIPPED: Could not start uvx mcp-server-time: ${e.message}")
    null
}

if (process != null) {
    val writer = process.outputStream.bufferedWriter()
    val reader = process.inputStream.bufferedReader()
    var reqId = 0

    fun send(method: String, params: String?, expectResponse: Boolean): String? {
        val id = if (expectResponse) ++reqId else null
        val idStr = if (id != null) """"id":$id,""" else ""
        writer.write("""{"jsonrpc":"2.0",${idStr}"method":"$method","params":${params ?: "null"}}""" + "\n")
        writer.flush()
        return if (expectResponse) reader.readLine() else null
    }

    // MCP initialize handshake — if the response is null the server failed to start
    val initResponse = send(
        "initialize",
        """{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"monty-test","version":"0.1"}}""",
        true
    )

    if (initResponse != null) {
        send("notifications/initialized", null, false)

        // Python code that calls the MCP get_current_time tool as an external function
        val code = """
result = get_current_time(timezone="UTC")
result
""".trimIndent()

        val monty = MontyKt.create(code, null, null, listOf("get_current_time"), null)

        val result = monty.run("{}", object : ExternalFunctionHandler {
            override fun call(functionName: String, argsJson: String, kwargsJson: String): String {
                // Extract timezone from kwargs JSON (e.g. {"timezone":"UTC"})
                val tz = Regex(""""timezone"\s*:\s*"([^"]+)"""").find(kwargsJson)?.groupValues?.get(1) ?: "UTC"
                val response = send(
                    "tools/call",
                    """{"name":"$functionName","arguments":{"timezone":"$tz"}}""",
                    true
                )!!
                // Extract "text" field from:
                // {"jsonrpc":"2.0","id":N,"result":{"content":[{"type":"text","text":"..."}]}}
                // Use a multiline-capable match since the text may contain \n sequences.
                val text = Regex(""""text"\s*:\s*"((?:[^"\\]|\\.)*)"""").find(response)?.groupValues?.get(1) ?: ""
                // Unescape JSON escape sequences, then re-encode as a proper JSON string.
                val unescaped = text.replace("\\n", "\n").replace("\\r", "\r").replace("\\t", "\t").replace("\\\"", "\"").replace("\\\\", "\\")
                val escaped = unescaped.replace("\\", "\\\\").replace("\"", "\\\"").replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t")
                return "\"$escaped\""
            }
        }, null)

        println("MCP time result: $result")
        assert(result.isNotEmpty()) { "Expected non-empty result from MCP time server, got: $result" }
        println("MCP test passed!")
    } else {
        println("SKIPPED: mcp-server-time did not respond to initialize (server unavailable)")
    }

    process.destroy()
}
