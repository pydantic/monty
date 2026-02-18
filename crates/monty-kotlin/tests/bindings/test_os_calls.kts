import uniffi.monty_kotlin.*

// The Kotlin binding blocks all OS-level calls (filesystem, environment) and
// raises OsCallNotSupported when sandboxed Python code tries to use them.
// These tests correspond to tests in test_os_calls.py and test_os_access.py
// that verify OS operations are correctly intercepted.

val noopHandler = object : ExternalFunctionHandler {
    override fun call(functionName: String, argsJson: String, kwargsJson: String): String = "null"
}

// === Path.exists() raises OsCallNotSupported ===
// From test_os_calls.py::test_path_exists_yields_oscall (error path in Kotlin binding)
try {
    MontyKt.create(
        """from pathlib import Path; Path("/tmp/test.txt").exists()""",
        null, null, null, null
    ).run("{}", noopHandler, null)
    throw RuntimeException("Expected OsCallNotSupported for Path.exists()")
} catch (e: MontyException.OsCallNotSupported) {
    assert(e.reason.isNotEmpty()) { "Expected non-empty reason, got: ${e.reason}" }
}

// === Path.stat() raises OsCallNotSupported ===
// From test_os_calls.py::test_path_stat_yields_oscall
try {
    MontyKt.create(
        """from pathlib import Path; Path("/etc/passwd").stat()""",
        null, null, null, null
    ).run("{}", noopHandler, null)
    throw RuntimeException("Expected OsCallNotSupported for Path.stat()")
} catch (e: MontyException.OsCallNotSupported) {
    assert(e.reason.isNotEmpty()) { "Expected non-empty reason, got: ${e.reason}" }
}

// === Path.read_text() raises OsCallNotSupported ===
// From test_os_calls.py::test_path_read_text_yields_oscall
try {
    MontyKt.create(
        """from pathlib import Path; Path("/tmp/hello.txt").read_text()""",
        null, null, null, null
    ).run("{}", noopHandler, null)
    throw RuntimeException("Expected OsCallNotSupported for Path.read_text()")
} catch (e: MontyException.OsCallNotSupported) {
    assert(e.reason.isNotEmpty()) { "Expected non-empty reason, got: ${e.reason}" }
}

// === Path concatenation then .exists() raises OsCallNotSupported ===
// From test_os_calls.py::test_path_concatenation (verifies path ops reach OS call)
try {
    MontyKt.create("""
from pathlib import Path
base = Path('/home')
full = base / 'user' / 'documents' / 'file.txt'
full.exists()
""".trimIndent(), null, null, null, null).run("{}", noopHandler, null)
    throw RuntimeException("Expected OsCallNotSupported after path concatenation")
} catch (e: MontyException.OsCallNotSupported) {
    assert(e.reason.isNotEmpty()) { "Expected non-empty reason, got: ${e.reason}" }
}

// === Path.write_text() raises OsCallNotSupported ===
// From test_os_calls.py::test_path_write_text_yields_oscall
try {
    MontyKt.create(
        """from pathlib import Path; Path("/tmp/output.txt").write_text("hello world")""",
        null, null, null, null
    ).run("{}", noopHandler, null)
    throw RuntimeException("Expected OsCallNotSupported for Path.write_text()")
} catch (e: MontyException.OsCallNotSupported) {
    assert(e.reason.isNotEmpty()) { "Expected non-empty reason, got: ${e.reason}" }
}

// === Path.mkdir() raises OsCallNotSupported ===
// From test_os_calls.py::test_path_mkdir_yields_oscall
try {
    MontyKt.create(
        """from pathlib import Path; Path("/tmp/newdir").mkdir()""",
        null, null, null, null
    ).run("{}", noopHandler, null)
    throw RuntimeException("Expected OsCallNotSupported for Path.mkdir()")
} catch (e: MontyException.OsCallNotSupported) {
    assert(e.reason.isNotEmpty()) { "Expected non-empty reason, got: ${e.reason}" }
}

// === Path.unlink() raises OsCallNotSupported ===
// From test_os_calls.py::test_path_unlink_yields_oscall
try {
    MontyKt.create(
        """from pathlib import Path; Path("/tmp/to_delete.txt").unlink()""",
        null, null, null, null
    ).run("{}", noopHandler, null)
    throw RuntimeException("Expected OsCallNotSupported for Path.unlink()")
} catch (e: MontyException.OsCallNotSupported) {
    assert(e.reason.isNotEmpty()) { "Expected non-empty reason, got: ${e.reason}" }
}

// === os.getenv() raises OsCallNotSupported ===
// From test_os_calls.py::test_os_getenv_yields_oscall
try {
    MontyKt.create(
        """import os; os.getenv("HOME")""",
        null, null, null, null
    ).run("{}", noopHandler, null)
    throw RuntimeException("Expected OsCallNotSupported for os.getenv()")
} catch (e: MontyException.OsCallNotSupported) {
    assert(e.reason.isNotEmpty()) { "Expected non-empty reason, got: ${e.reason}" }
}

// === os.getenv() with default raises OsCallNotSupported ===
// From test_os_calls.py::test_os_getenv_with_default_yields_oscall
try {
    MontyKt.create(
        """import os; os.getenv("MISSING", "fallback")""",
        null, null, null, null
    ).run("{}", noopHandler, null)
    throw RuntimeException("Expected OsCallNotSupported for os.getenv() with default")
} catch (e: MontyException.OsCallNotSupported) {
    assert(e.reason.isNotEmpty()) { "Expected non-empty reason, got: ${e.reason}" }
}

// === os.environ raises OsCallNotSupported ===
// From test_os_calls.py::test_os_environ_yields_oscall
try {
    MontyKt.create(
        """import os; os.environ""",
        null, null, null, null
    ).run("{}", noopHandler, null)
    throw RuntimeException("Expected OsCallNotSupported for os.environ")
} catch (e: MontyException.OsCallNotSupported) {
    assert(e.reason.isNotEmpty()) { "Expected non-empty reason, got: ${e.reason}" }
}

// === Multiple OS calls in sequence — first one raises OsCallNotSupported ===
// From test_os_calls.py::test_multiple_path_calls (error path in Kotlin binding)
try {
    MontyKt.create("""
from pathlib import Path
p = Path('/tmp/test.txt')
exists = p.exists()
is_file = p.is_file()
(exists, is_file)
""".trimIndent(), null, null, null, null).run("{}", noopHandler, null)
    throw RuntimeException("Expected OsCallNotSupported for sequential OS calls")
} catch (e: MontyException.OsCallNotSupported) {
    assert(e.reason.isNotEmpty()) { "Expected non-empty reason, got: ${e.reason}" }
}

// === Path operations in conditionals also raise OsCallNotSupported ===
// From test_os_calls.py::test_conditional_path_calls
try {
    MontyKt.create("""
from pathlib import Path
p = Path('/tmp/test.txt')
if p.exists():
    content = p.read_text()
else:
    content = 'not found'
content
""".trimIndent(), null, null, null, null).run("{}", noopHandler, null)
    throw RuntimeException("Expected OsCallNotSupported for OS call inside conditional")
} catch (e: MontyException.OsCallNotSupported) {
    assert(e.reason.isNotEmpty()) { "Expected non-empty reason, got: ${e.reason}" }
}
