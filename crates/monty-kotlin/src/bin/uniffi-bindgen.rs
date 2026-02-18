/// Entry point for the uniffi-bindgen CLI.
///
/// This binary delegates entirely to UniFFI's built-in `uniffi_bindgen_main` to generate
/// language bindings (Kotlin, Swift, Python, etc.) from the compiled native library.
///
/// Usage (from workspace root):
/// ```bash
/// cargo run -p monty-kotlin --features uniffi-bindgen-cli --bin uniffi-bindgen -- \
///   generate --library target/release/libmonty_kotlin.dylib --language kotlin \
///   --out-dir crates/monty-kotlin/kotlin/src/main/kotlin
/// ```
fn main() {
    uniffi::uniffi_bindgen_main()
}
