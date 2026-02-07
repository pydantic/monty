//! Fuzz target for testing that arbitrary Python code doesn't cause panics or crashes.
//!
//! This target feeds arbitrary byte sequences to the Monty interpreter and verifies that
//! neither parsing nor execution causes the interpreter to panic or crash. Errors (parse
//! errors, runtime errors, etc.) are expected and ignored - we only care about panics.
#![no_main]

use libfuzzer_sys::fuzz_target;
use monty::MontyRun;

fuzz_target!(|code: String| {
    // Try to parse the code
    let Ok(runner) = MontyRun::new(
        code.to_owned(),
        "fuzz.py",
        vec![], // no inputs
        vec![], // no external functions
    ) else {
        return; // Parse errors are expected for random input
    };

    // Try to execute - ignore runtime errors, we only care about panics/crashes
    let _ = runner.run_no_limits(vec![]);
});
