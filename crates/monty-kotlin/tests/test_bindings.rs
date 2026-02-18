// UniFFI foreign language test runner for the Kotlin bindings.
// Compiles and runs the `.kts` test scripts using `build_foreign_language_testcases!`.
uniffi::build_foreign_language_testcases!(
    "tests/bindings/test_basic.kts",
    "tests/bindings/test_mcp.kts",
);
