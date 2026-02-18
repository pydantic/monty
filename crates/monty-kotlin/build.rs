/// Build script that generates UniFFI scaffolding from the UDL file.
///
/// This is required by uniffi's hybrid UDL + proc-macro approach: the UDL declares
/// the namespace, and proc-macros on the Rust types handle everything else.
fn main() {
    uniffi::generate_scaffolding("src/monty_kotlin.udl").unwrap();
}
