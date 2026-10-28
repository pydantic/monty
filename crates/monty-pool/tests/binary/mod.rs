mod build;

use std::{env, path::PathBuf, sync::OnceLock};

/// Locates and builds the `monty` CLI once per test binary.
pub fn monty_binary() -> PathBuf {
    static BINARY: OnceLock<PathBuf> = OnceLock::new();

    if let Some(path) = env::var_os("MONTY_TEST_BIN") {
        return PathBuf::from(path);
    }
    BINARY
        .get_or_init(|| build::binary(&["-p", "monty-runtime"], "monty"))
        .clone()
}
