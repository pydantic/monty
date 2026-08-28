//! The `copy` module's native-stack budget; behaviour lives in
//! `test_cases/copy__all.py`, where it runs against CPython too.
//!
//! `deepcopy` recurses on the Rust stack, one recursion level per step, so what
//! the limit really bounds is bytes of native stack. Measured in release, a step
//! costs ~770 bytes for a list and ~1.1 KiB for a dict or an instance — so a
//! near-limit list copy fits a worker's stack and a dict or instance chain of
//! the same depth does not, exceeding 1 MiB at around depth 950. That is the
//! exposure every Rust-side walk has (`py_eq` needs ~1.03 MiB at depth 980), and
//! it goes away when `RunError` stops costing 96 bytes in every frame.
//!
//! These tests pin what is true today: the limit fires before the stack for the
//! shape that fits, and each shape's cost per level stays where it is.

use std::thread;

use monty::MontyRun;
use monty_types::{CompileOptions, ExcType, MontyException, PrintWriter, ResourceLimits, ResourceTracker};

/// The tightest stack a worker actually runs on: the wasm module's 1 MiB
/// default. Native workers run on the process main thread's 8 MiB, and
/// libtest's own 8 MiB would hide an overshoot that aborts in production.
#[cfg(not(debug_assertions))]
const WORKER_STACK: usize = 1024 * 1024;

/// A debug build is what CI runs these on, and its frames are several times
/// fatter than release's, so the debug budget is scaled rather than the depths
/// reduced — the assertion worth keeping is the release one. Generous because
/// the multiplier is not constant across targets, and a target whose frames are
/// half again as fat must not abort the test binary.
#[cfg(debug_assertions)]
const WORKER_STACK: usize = 8 * 1024 * 1024;

/// Nesting just inside the recursion limit, where the copy must still succeed.
fn depth_within_the_limit() -> usize {
    ResourceLimits::default().max_recursion_depth - 40
}

/// The depth the shapes costing ~1.1 KiB a level are pinned at: enough to catch
/// a regression that inflates the per-level cost, low enough to fit the worker
/// stack they currently overrun near the limit.
const DEPTH_WITHIN_THE_WORKER_STACK: usize = 600;

/// Deep-copying past the recursion limit must raise `RecursionError`, not
/// overflow the native stack.
#[test]
fn deep_copy_past_the_recursion_limit_raises() {
    let error = run_on_worker_stack(
        r"
import copy

x = []
for _ in range(5000):
    x = [x]
copy.deepcopy(x)
",
    )
    .expect_err("nesting past the recursion limit should raise");
    assert_eq!(error.exc_type(), ExcType::RecursionError);
}

/// Nesting just under that depth must still copy, on the same budget — the
/// guard against "fixing" an overflow by refusing to go deep.
#[test]
fn deep_copy_of_nested_lists_just_under_the_limit_succeeds() {
    let depth = depth_within_the_limit();
    let code = format!(
        r"
import copy

x = []
for _ in range({depth}):
    x = [x]
y = copy.deepcopy(x)

depth = 0
while y:
    y = y[0]
    depth += 1
assert depth == {depth}
"
    );
    run_on_worker_stack(&code).expect("a copy within the recursion limit should succeed");
}

/// A dict level costs half again what a list level does, the copy running
/// through `deep_copy_pair` on the way to each value.
#[test]
fn deep_copy_of_nested_dicts_stays_within_a_worker_stack() {
    let depth = DEPTH_WITHIN_THE_WORKER_STACK;
    let code = format!(
        r"
import copy

x = {{}}
for _ in range({depth}):
    x = {{'inner': x}}
y = copy.deepcopy(x)

depth = 0
while y:
    y = y['inner']
    depth += 1
assert depth == {depth}
"
    );
    run_on_worker_stack(&code).expect("a copy within the stack budget should succeed");
}

/// The most expensive shape there is: an instance level adds
/// `deep_copy_instance` and `deep_copy_attrs` on top of what a dict costs.
#[test]
fn deep_copy_of_nested_instances_stays_within_a_worker_stack() {
    let depth = DEPTH_WITHIN_THE_WORKER_STACK;
    let code = format!(
        r"
import copy


class Node:
    def __init__(self, inner):
        self.inner = inner


x = None
for _ in range({depth}):
    x = Node(x)
y = copy.deepcopy(x)

depth = 0
while y is not None:
    y = y.inner
    depth += 1
assert depth == {depth}
"
    );
    run_on_worker_stack(&code).expect("a copy within the stack budget should succeed");
}

/// Runs `code` on a worker-sized stack, so an overshoot fails here.
fn run_on_worker_stack(code: &str) -> Result<(), MontyException> {
    let code = code.to_owned();
    thread::Builder::new()
        .stack_size(WORKER_STACK)
        .spawn(move || {
            let runner = MontyRun::new(code, "test.py", vec![], CompileOptions::default()).expect("code compiles");
            runner
                .run(vec![], ResourceTracker::default(), PrintWriter::Stdout)
                .map(|_| ())
        })
        .expect("spawning the bounded-stack thread")
        .join()
        .expect("the copy must not abort the process")
}
