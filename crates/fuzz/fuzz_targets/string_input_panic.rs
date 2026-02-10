//! Fuzz target for testing that arbitrary Python code doesn't cause panics or crashes.
//!
//! This target feeds arbitrary byte sequences to the Monty interpreter and verifies that
//! neither parsing nor execution causes the interpreter to panic or crash. Errors (parse
//! errors, runtime errors, etc.) are expected and ignored - we only care about panics.
//!
//! Resource limits are enforced to prevent infinite loops and memory exhaustion.
#![no_main]

use std::time::Duration;

use libfuzzer_sys::fuzz_target;
use monty::{LimitedTracker, MontyRun, NoPrint, ResourceLimits};

/// Resource limits for fuzzing - restrictive to prevent hangs and memory issues.
fn fuzz_limits() -> LimitedTracker {
    LimitedTracker::new(
        ResourceLimits::new()
            .max_allocations(10_000)
            .max_memory(1024 * 1024) // 1 MB
            .max_duration(Duration::from_millis(100)),
    )
}

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

    // Try to execute with resource limits - ignore all errors, we only care about panics/crashes
    let _ = runner.run(vec![], fuzz_limits(), &mut NoPrint);
});
