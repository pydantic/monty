# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Monty is a sandboxed Python interpreter written in Rust. It parses Python code using Ruff's `ruff_python_parser` but implements its own runtime execution model for safety and performance. This is a work-in-progress project that currently supports a subset of Python features.

Project goals:

- **Safety**: Execute untrusted Python code safely without FFI or C dependencies, instead sandbox will call back to host to run foreign/external functions.
- **Performance**: Fast execution through compile-time optimizations and efficient memory layout
- **Simplicity**: Clean, understandable implementation focused on a Python subset
- **Snapshotting and iteration**: Plan is to allow code to be iteratively executed and snapshotted at each function call
- Targets the latest stable version of Python, currently Python 3.14

## Important Security Notice

It's ABSOLUTELY CRITICAL that there's no way for code run in a Monty sandbox to access the host filesystem, or environment or to in any way "escape the sandbox".

**Monty will be used to run untrusted, potentially malicious code.**

Make sure there's no risk of this, either in the implementation, or in the public API that makes it more like that a developer using the pydantic_monty package might make such a mistake.

Possible security risks to consider:
* filesystem access
* path traversal to access files the users did not intend to expose to the monty sandbox
* memory errors - use of unsafe memory operations
* excessive memory usage - evading monty's resource limits
* infinite loops - evading monty's resource limits
* network access - sockets, HTTP requests
* subprocess/shell execution - os.system, subprocess, etc.
* import system abuse - importing modules with side effects or accessing `__import__`
* external function/callback misuse - callbacks run in host environment
* deserialization attacks - loading untrusted serialized Monty/snapshot data
* regex/string DoS - catastrophic backtracking or operations bypassing limits
* information leakage via timing or error messages
* Python/Javascript/Rust APIs that accidentally allow developers to expose their host to monty code

## Bytecode VM Architecture

Monty is implemented as a bytecode VM, same as CPython.

### Reference Count Safety

All types that implement `DropWithHeap` hold heap references and **must** be cleaned up correctly on every code path — not just the happy path, but also early returns via `?`, `continue`, conditional branches, etc. A missed `drop_with_heap` on any branch leaks reference counts. There are three mechanisms for ensuring this, listed in order of preference:

#### 1. `defer_drop!` macro (preferred)

The simplest and safest approach. Use `defer_drop!` (or `defer_drop_mut!` when mutable access to the value is needed) to bind a value into a guard that automatically drops it when scope exits — whether that's normal completion, early return via `?`, `continue`, or any other branch. The macro rebinds the value and heap variables as borrows from the guard, so you keep using them by name as before:

```rust
let value = self.pop();
defer_drop!(value, heap);          // value is now &Value, heap is now &mut Heap
let result = value.py_repr(heap)?; // guard handles cleanup on all paths
```

Beyond safety, `defer_drop!` is often much more concise than inserting `drop_with_heap` calls in every branch of complex control flow.

`defer_drop!` gives you an immutable reference to the value. Use `defer_drop_mut!` when you need a mutable reference (e.g. iterators, values you may swap):

```rust
let iter = heap.get_iter(iter_ref);
defer_drop_mut!(iter, heap);
while let Some(item) = iter.for_next(heap)? { ... }
```

**Limitation:** because the macro rebinds the heap, it cannot be used inside `&mut self` methods where `self` owns the heap — first assign `let this = self;` and pass `this` instead.

#### 2. `HeapGuard` (when you need control over the value's fate)

Use `HeapGuard` directly when `defer_drop!` is too restrictive — specifically when you need to conditionally extract the value instead of dropping it. `HeapGuard` provides `into_inner()` and `into_parts()` to reclaim ownership, while its `Drop` impl still guarantees cleanup on all other paths:

```rust
// HeapGuard needed here because on success we push lhs back onto the stack
// instead of dropping it
let mut lhs_guard = HeapGuard::new(self.pop(), self);
let (lhs, this) = lhs_guard.as_parts_mut();

if lhs.py_iadd(rhs, this.heap)? {
    let (lhs, this) = lhs_guard.into_parts(); // reclaim lhs, don't drop
    this.push(lhs);
    return Ok(());
}
// otherwise lhs_guard drops lhs automatically at scope exit
```

#### 3. Manual `drop_with_heap` (for trivially simple cases)

For very simple cases with a single linear code path and no branching between acquiring and releasing the value, a direct `drop_with_heap` call is fine:

```rust
let iter = self.pop();
iter.drop_with_heap(&mut self.heap); // single path, no branching
```

Avoid manual `drop_with_heap` whenever there are multiple code paths (branching, `?`, `continue`, early returns) between acquiring and releasing the value — that is exactly where `defer_drop!` or `HeapGuard` prevent leaks by guaranteeing cleanup on every path.

## Dev Commands

DO NOT run `cargo build` or `cargo run`, it will fail because of issues with Python bindings.

Instead use the following `make` commands:

```bash
make install-py           Install python dependencies
make install-js           Install JS package dependencies
make install              Install the package, dependencies, and pre-commit for local development
make dev-py               Install the python package for development
make dev-js               Build the JS package (debug)
make lint-js              Lint JS code with oxlint
make test-js              Build and test the JS package
make dev-py-release       Install the python package for development with a release build
make dev-js-release       Build the JS package (release)
make dev-py-pgo           Install the python package for development with profile-guided optimization
make format-rs            Format Rust code with fmt
make format-py            Format Python code - WARNING be careful about this command as it may modify code and break tests silently!
make format-js            Format JS code with prettier
make format               Format Rust code, this does not format Python code as we have to be careful with that
make lint-rs              Lint Rust code with clippy and import checks
make clippy-fix           Fix Rust code with clippy
make lint-py              Lint Python code with ruff
make lint                 Lint the code with ruff and clippy
make format-lint-rs       Format and lint Rust code with fmt and clippy
make format-lint-py       Format and lint Python code with ruff
make test-no-features     Run rust tests without any features enabled
make test-ref-count-panic Run rust tests with ref-count-panic enabled
make test-ref-count-return Run rust tests with ref-count-return enabled
make test-cases           Run tests cases only
make test-type-checking   Run rust tests on monty_type_checking
make pytest               Run Python tests with pytest
make test-py              Build the python package (debug profile) and run tests
make test-docs            Test docs examples only
make test                 Run rust tests
make testcov              Run Rust tests with coverage, print table, and generate HTML report
make complete-tests       Fill in incomplete test expectations using CPython
make update-typeshed      Update vendored typeshed from upstream
make bench                Run benchmarks
make dev-bench            Run benchmarks to test with dev profile
make profile              Profile the code with pprof and generate flamegraphs
make type-sizes           Write type sizes for the crate to ./type-sizes.txt (requires nightly and top-type-sizes)
make main                 run linting and the most important tests
make help                 Show this help (usage: make help)
```

Use the /python-playground skill to check cpython and monty behavior.

## Releasing

See [RELEASING.md](RELEASING.md) for the release process.

## Exception

It's important that exceptions raised/returned by this library match those raised by Python.

Wherever you see an Exception with a repeated message, create a dedicated method to create that exception `src/exceptions.rs`.

When writing exception messages, always check `src/exceptions.rs` for existing methods to generate that message.

## Code style

Avoid local imports, unless there's a very good reason, all imports should be at the top of the file.

Avoid `fn my_func<T: MyTrait>(..., param: T)` style function definitions, STRONGLY prefer `fn my_func(param: impl MyTrait)` syntax since changes are more localized. This includes in trait definitions and implementations.

Also avoid using functions and structs via a path like `std::borrow::Cow::Owned(...)`, instead import `Cow` globally with `use std::borrow::Cow;`.

NEVER use `allow()` in rust lint markers, instead use `expect()` so any unnecessary markers are removed. E.g. use

```rs
#[expect(clippy::too_many_arguments)]
```

NOT!

```rs
#[allow(clippy::too_many_arguments)]
```

### Docstrings and comments.

IMPORTANT: every struct, enum and function should be a comprehensive but concise docstring to
explain what it does and why and any considerations or potential foot-guns of using that type.

The only exception is trait implementation methods where a docstring is not necessary if the method is self-explanatory.

It's important that docstrings cover the motivation and primary usage patterns of code, not just the simple "what it does".

Similarly, you should add comments to code, especially if the code is complex or esoteric.

Only add examples to docstrings of public functions and structs, examples should be <=8 lines, if the example is more, remove it.

If you add example code to docstrings, it must be run in tests. NEVER add examples that are ignored.

If you encounter a comment or docstring that's out of date - you MUST update it to be correct.

Similarly, if you encounter code that has no docstrings or comments, or they are minimal, you should add more detail.

NOTE: COMMENTS AND DOCSTRINGS ARE EXTREMELY IMPORTANT TO THE LONG TERM HEALTH OF THE PROJECT.

## Tests

Do **NOT** write tests within modules unless explicitly prompted to do so.

Tests should live in the relevant `tests/` directory.

Commands:

```bash
# Build the project
cargo build

# Run tests (this is the best way to run all tests as it enables the ref-count-panic feature)
make test-ref-count-panic

# Run crates/monty/test_cases tests only
make test-cases

# Run a specific test
cargo test -p monty --test datatest_runner --features ref-count-panic str__ops

# Run the interpreter on a Python file
cargo run -- <file.py>
```

See more test commands above.

### Experimentation and Playground

Read `Makefile` for other useful commands.

DO NOT run `cargo run --`, it will fail because of issues with Python bindings.

You can use the `./playground` directory (excluded from git, create with `mkdir -p playground`) to write files
when you want to experiment by running a file with cpython or monty, e.g.:
* `python3 playground/test.py` to run the file with cpython
* `cargo run -- playground/test.py` to run the file with monty

DO NOT use `/tmp` or pipe code to the interpreter as it requires extra permissions and can slow you down!

More details in the "python-playground" skill.

### Test File Structure

Most functionality should be tested via python files in the `crates/monty/test_cases` directory.

**DO NOT create many small test files.** This would be unmaintainable.

ALWAYS consolidate related tests into single files using multiple `assert` statements. Follow `crates/monty/test_cases/fstring__all.py` as the gold standard pattern:

```python
# === Section name ===
# brief comment if needed
assert condition, 'descriptive message'
assert another_condition, 'another descriptive message'

# === Next section ===
x = setup_value
assert x == expected, 'test description'
```

Each `assert` should have a descriptive message.

Do NOT Write tests like `assert 'thing' in msg` it's lazy and inexact unless explicitly told to do so, instead write tests like `assert msg == 'expected message'` to ensure clarity and accuracy and most importantly, to identify differences between Monty and CPython.

### When to Create Separate Test Files

Only create a separate test file when you MUST use one of these special expectation formats:

- `"""TRACEBACK:..."""` - Test expects an exception with full traceback (PREFERRED for error tests)
- `# Raise=Exception('message')` - Test expects an exception without traceback verification - NOT RECOMMENDED, use `TRACEBACK` instead
- `# ref-counts={...}` - Test checks reference counts (special mode)
- you're writing tests for a different behavior or section of the language

For everything else, **add asserts to an existing test file** or create ONE consolidated file for the feature.

### File Naming

Name files by feature, not by micro-variant:
- ✅ `str__ops.py` - all string operations (add, iadd, len, etc.)
- ✅ `list__methods.py` - all list method tests
- ❌ `str__add_basic.py`, `str__add_empty.py`, `str__add_multiple.py` - TOO GRANULAR

### Expectation Formats (use sparingly)

Only use these when `assert` won't work (on last line of file):
- `# Return=value` - Check `repr()` output (prefer assert instead)
- `# Return.str=value` - Check `str()` output (prefer assert instead)
- `# Return.type=typename` - Check `type()` output (prefer assert instead)
- `# Raise=Exception('message')` - Expect exception without traceback (REQUIRES separate file)
- `"""TRACEBACK:..."""` - Expect exception with full traceback (PREFERRED over `# Raise=`)
- `# ref-counts={...}` - Check reference counts (REQUIRES separate file)
- No expectation comment - Assert-based test (PREFERRED)

Do NOT use `# Return=` when you could use `assert` instead

### Traceback Tests (Preferred for Errors)

For tests that expect exceptions, **prefer traceback tests over `# Raise=`** because they verify:
- The full traceback with all stack frames
- Correct line numbers for each frame
- Function names in the traceback
- The caret markers (`~`) pointing to the error location

Traceback test format - add a triple-quoted string at the end of the file starting with `\nTRACEBACK:`:
```python
def foo():
    raise ValueError('oops')

foo()
"""
TRACEBACK:
Traceback (most recent call last):
  File "my_test.py", line 4, in <module>
    foo()
    ~~~~~
  File "my_test.py", line 2, in foo
    raise ValueError('oops')
ValueError: oops
"""
```

Key points:
- The filename in the traceback should match the test file name (just the basename, not the full path)
- Use `~` for caret markers (the test runner normalizes CPython's `^` to `~`)
- The `<module>` frame name is used for top-level code
- Tests run against both Monty and CPython, so the traceback must match both

Only use `# Raise=` when you only care about the exception type/message and not the traceback.

### Python fixture markers

You may mark python files with:
* `# call-external` to support calling external functions
* `# run-async` to support running async code

NEVER MARK TESTS AS XFAIL UNDER ANY CIRCUMSTANCES!!! INSTEAD FIX THE BEHAVIOR SO THAT THE TEST PASSES.

Never mark tests as:
- `# xfail=cpython` - Test is required to fail on CPython
- `# xfail=monty` - Test is required to fail on Monty

NEVER MARK TESTS AS XFAIL UNDER ANY CIRCUMSTANCES!!! INSTEAD FIX THE BEHAVIOR SO THAT THE TEST PASSES.

All these markers must be at the start of comment lines to be recognized.

### Other Notes

- Prefer single quotes for strings in Python tests
- Do NOT add `# noqa` or  `# pyright: ignore` comments to test code, instead add the failing code to `pyproject.toml`
- The ONLY exception is `await` expressions outside of async functions, where you should add `# pyright: ignore`
- Run `make lint-py` after adding tests
- Use `make complete-tests` to fill in blank expectations
- Tests run via `datatest-stable` harness in `tests/datatest_runner.rs`, use `make test-cases` to run them

## Python Package (`pydantic-monty`)

The Python package provides Python bindings for the Monty interpreter, located in `crates/monty-python/`.

### Structure

- `crates/monty-python/src/` - Rust source for PyO3 bindings
- `crates/monty-python/python/pydantic_monty/_monty.pyi` - Type stubs for the Python module
- `crates/monty-python/tests/` - Python tests using pytest

### Building and Testing

Dependencies needed for python testing are installed in `crates/monty-python/pyproject.toml`.
To install these dependencies, use `uv sync --all-packages --only-dev`.

```bash
# Build the Python package for development (required before running tests)
make dev-py

# Run Python tests
make test-py

# Or run pytest directly (after dev-py)
uv run pytest

# Run a specific test file
uv run pytest crates/monty-python/tests/test_basic.py

# Run a specific test
uv run pytest crates/monty-python/tests/test_basic.py::test_simple_expression
```

### Python Test Guidelines

Check and follow the style of other python tests.

Make sure you put tests in the correct file.

**DO NOT use python/pytest tests for `monty` core functionality!** When testing core functionality, add tests to `crates/monty/test_cases/` or `crates/monty/tests/`. Only use python/pytest tests for `pydantic_monty` functionality testing.

**NEVER use class-based tests.** All tests should be simple functions.

Use `@pytest.mark.parametrize` whenever testing multiple similar cases.

Use `snapshot` from `inline-snapshot` for all test asserts.

NEVER do the lazy `assert '...' in ...` instead always do `assert value == snapshot()`,
then run the test and inline-snapshot will fill in the missing value in the `snapshot()` call.

Use `pytest.raises` for expected exceptions, like this

```py
with pytest.raises(ValueError) as exc_info:
    m.run(print_callback=callback)
assert exc_info.value.args[0] == snapshot('stopped at 3')
```

## Reference Counting

Heap-allocated values (`Value::Ref`) use manual reference counting. Key rules:

- **Cloning**: Use `clone_with_heap(heap)` which increments refcounts for `Ref` variants.
- **Dropping**: Call `drop_with_heap(heap)` when discarding an `Value` that may be a `Ref`.

Container types (`List`, `Tuple`, `Dict`) also have `clone_with_heap()` methods.

**Resource limits**: When resource limits (allocations, memory, time) are exceeded, execution terminates with a `ResourceError`. No guarantees are made about the state of the heap or reference counts after a resource limit is exceeded. The heap may contain orphaned objects with incorrect refcounts. This is acceptable because resource exhaustion is a terminal error - the execution context should be discarded.

## NOTES

ALWAYS consider code quality when adding new code, if functions are getting too complex or code is duplicated, move relevant logic to a new file.
Make sure functions are added in the most logical place, e.g. as methods on a struct where appropriate.

The code should follow the "newspaper" style where public and primary functions are at the top of the file, followed by private functions and utilities.
ALWAYS put utility, private functions and "sub functions" underneath the function they're used in.

It is important to the long term health of the project and maintainability of the codebase that code is well structured and organized, this is very important.

ALWAYS run `make format-rs` and `make lint-rs` after making changes to rust code and fix all suggestions to maintain code quality.

ALWAYS run `make lint-py` after making changes to python code and fix all suggestions to maintain code quality.

ALWAYS update this file when it is out of date.

NEVER add imports anywhere except at the top of the file, this applies to both python and rust.

NEVER write `unsafe` code, if you think you need to write unsafe code, explicitly ask the user or leave a `todo!()` with a suggestion and explanation.

## JavaScript Package (`monty-js`)

The JavaScript package provides Node.js bindings for the Monty interpreter via napi-rs, located in `crates/monty-js/`.

### Structure

- `crates/monty-js/src/lib.rs` - Rust source for napi-rs bindings
- `crates/monty-js/index.js` - Auto-generated JS loader that detects platform and loads the appropriate native binding
- `crates/monty-js/index.d.ts` - TypeScript type declarations (auto-generated)
- `crates/monty-js/__test__/` - Tests using ava

### Current API

The package exposes:

- `Monty` class - Parse and execute Python code with inputs, external functions, and resource limits
- `MontySnapshot` / `MontyComplete` - For iterative execution with `start()` / `resume()`
- `runMontyAsync()` - Helper for async external functions
- `MontySyntaxError` / `MontyRuntimeError` / `MontyTypingError` - Error classes

```ts
import { Monty, MontySnapshot, runMontyAsync } from '@pydantic/monty'

// Basic execution
const m = new Monty('x + 1', { inputs: ['x'] })
const result = m.run({ inputs: { x: 10 } }) // returns 11

// Iterative execution for external functions
const m2 = new Monty('fetch(url)', { inputs: ['url'], externalFunctions: ['fetch'] })
let progress = m2.start({ inputs: { url: 'https://...' } })
if (progress instanceof MontySnapshot) {
  progress = progress.resume({ returnValue: 'response data' })
}
```

See `crates/monty-js/README.md` for full API documentation.

### Building and Testing

```bash
# Install dependencies
make install-js

# Build native binding (debug)
make build-js

# Build native binding (release)
make build-js-release

# Run tests
make test-js

# Format JavaScript code
make format-js

# Lint JavaScript code
make lint-js
```

Or run directly in `crates/monty-js`:

```bash
npm install
npm run build        # release build
npm run build:debug  # debug build
npm test
```

### JavaScript Test Guidelines

- Tests use [ava](https://github.com/avajs/ava) and live in `crates/monty-js/__test__/`
- Tests are written in TypeScript
- Follow the existing test style in the `__test__/` directory
