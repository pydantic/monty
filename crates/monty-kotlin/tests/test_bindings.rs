// UniFFI foreign language test runner for the Kotlin bindings.
// Compiles and runs the `.kts` test scripts using `build_foreign_language_testcases!`.
uniffi::build_foreign_language_testcases!(
    "tests/bindings/test_basic.kts",
    "tests/bindings/test_types.kts",
    "tests/bindings/test_exceptions.kts",
    "tests/bindings/test_external.kts",
    "tests/bindings/test_type_check.kts",
    "tests/bindings/test_print.kts",
    "tests/bindings/test_os_calls.kts",
    "tests/bindings/test_limits.kts",
    "tests/bindings/test_mcp.kts",
);
