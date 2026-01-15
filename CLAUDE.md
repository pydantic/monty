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

## Bytecode VM Architecture

Monty is migrating from a recursive tree-walking interpreter to a stack-based bytecode VM. The full design is documented in `bytecode-plan.md`. This section summarizes the key architecture.

### Why Bytecode?

1. **Simplified pause/resume**: VM state is just IP + stacks (no complex snapshot machinery)
2. **Better performance**: Eliminates recursion, better cache locality
3. **Enables future JIT compilation**
4. **Enables future async support**

### Execution Model

The bytecode VM uses:

- **Operand stack**: Values being computed are pushed/popped from a stack (`Vec<Value>`)
- **Call frames**: Each function call creates a `CallFrame` with its own instruction pointer (IP)
- **IP per frame**: The instruction pointer lives in each `CallFrame`, not globally - this avoids sync bugs on call/return

```
┌─────────────────────────────────────────┐
│              VM State                    │
│  ┌─────────────────────────────────────┐│
│  │  Operand Stack: [val1, val2, ...]   ││
│  └─────────────────────────────────────┘│
│  ┌─────────────────────────────────────┐│
│  │  Call Frames:                        ││
│  │    Frame 0: { code, ip: 42, ... }   ││
│  │    Frame 1: { code, ip: 7, ... }    ││
│  └─────────────────────────────────────┘│
│  heap, namespaces, interns (existing)   │
└─────────────────────────────────────────┘
```

### Code Structure

New bytecode module (`src/bytecode/`):

- `mod.rs` - Module root
- `op.rs` - Opcode enum definitions (`#[repr(u8)]`, no data variants)
- `code.rs` - `Code` struct (bytecode, constants, location/exception tables)
- `compiler.rs` - AST to bytecode compilation
- `vm.rs` - Execution engine (main dispatch loop)
- `snapshot.rs` - VM state serialization for pause/resume

### Files Being Replaced

The following tree-walker files will be replaced by the bytecode VM:

- `src/evaluate.rs` - Tree-walking expression evaluator → bytecode compiler + VM
- `src/run_frame.rs` - Recursive frame execution → `CallFrame` stack in VM
- `src/snapshot.rs` - `SnapshotTracker`, `ClauseState`, `FunctionFrame` → simplified `VMSnapshot`

### What We Keep

- `Value` enum (16-byte hybrid design)
- `Heap<T>` with reference counting and free list
- `Interns` for string/bytes interning
- `Function` struct (stores metadata, will also store compiled `Code`)
- `Namespaces` stack (with modifications)

### Reference Count Safety

When operations can fail (return `Result`), operands must be dropped BEFORE propagating errors with `?`. Otherwise, reference counts leak:

```rust
// WRONG (leaks on error):
let result = lhs.py_add(&rhs, heap)?;  // If error, lhs/rhs leak!
lhs.drop_with_heap(heap);

// CORRECT (drop before propagating):
let result = lhs.py_add(&rhs, heap);   // Don't use ? yet
lhs.drop_with_heap(heap);              // Always drop operands
rhs.drop_with_heap(heap);
self.push(result?);                    // Now propagate error
```

## Dev Commands

DO NOT run `cargo build` or `cargo run`, it will fail because of issues with Python bindings.

Instead usee the following:

```bash
# lint python and rust code
make lint

# lint just rust code
make lint-rs

# lint just python code
make lint-py

# format python and rust code
make format

# format just rust code
make format-rs

# format just python code
make format-py
```

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

Only add examples to docstrings of public functions and structs, examples should be <=8 lines, if the example is more, remove it.

If you add example code to docstrings, it must be run in tests. NEVER add examples that are ignored.

Similarly, you should add lots of comments to code.

If you see a comment or docstring that's out of date - you MUST update it to be correct.

NOTE: COMMENTS AND DOCSTRINGS ARE EXTREMELY IMPORTANT TO THE LONG TERM HEALTH OF THE PROJECT.

## Tests

Do **NOT** write tests within modules unless explicitly prompted to do so.

Tests should live in the `tests/` directory.

Commands:

```bash
# Build the project
cargo build

# Run tests (this is the best way to run all tests as it enables the ref-count-panic feature)
make test-ref-count-panic

# Run a specific test
cargo test -p monty --test datatest_runner --features ref-count-panic str__ops

# Run test_cases tests only
make test-cases

# Run the interpreter on a Python file
cargo run -p monty-cli -- <file.py>
```

### Experimentation and Playground

Read `Makefile` for other useful commands.

DO NOT run `cargo run --`, it will fail because of issues with Python bindings.

You can use the `./playground` directory (excluded from git, create with `mkdir -p playground`) to write files
when you want to experiment by running a file with cpython or monty, e.g.:
* `python3 playground/test.py` to run the file with cpython
* `cargo run -p monty-cli -- playground/test.py` to run the file with monty

DO NOT use `/tmp` or pipeing code to the interpreter as it requires extra permissions and can slow you down!

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

### When to Create Separate Test Files

Only create a separate test file when you MUST use one of these special expectation formats:

- `"""TRACEBACK:..."""` - Test expects an exception with full traceback (PREFERRED for error tests)
- `# Raise=Exception('message')` - Test expects an exception without traceback verification
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

### Xfail Directive (Strict)

NEVER MARK TESTS AS XFAIL UNDER ANY CIRCUMSTANCES!!!

Optional, on first line of file - DO NOT use unless absolutely necessary:
- `# xfail=cpython` - Test is expected to fail on CPython (if it passes, that's an error)
- `# xfail=monty` - Test is expected to fail on Monty (if it passes, that's an error)

Xfail is **strict**: if a test marked xfail passes, the test runner will fail with an error
telling you to remove xfail since the test is now fixed.

NEVER MARK TESTS AS XFAIL UNDER ANY CIRCUMSTANCES!!!

### Other Notes

- Prefer single quotes for strings in Python tests
- do NOT add `# noqa` comments to test code, instead add the failing code to `pyproject.toml`
- Run `make lint-py` after adding tests
- Use `make complete-tests` to fill in blank expectations
- Tests run via `datatest-stable` harness in `tests/datatest_runner.rs`

## Python Package (`monty-python`)

The Python package provides Python bindings for the Monty interpreter, located in `crates/monty-python/`.

### Structure

- `crates/monty-python/src/` - Rust source for PyO3 bindings
- `crates/monty-python/monty.pyi` - Type stubs for the Python module
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

**NEVER use class-based tests.** All tests should be simple functions.

Use `@pytest.mark.parametrize` whenever testing multiple similar cases.

Use `snapshot` from `inline-snapshot` for all test asserts.

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
- **Borrow conflicts**: When you need to read from the heap and then mutate it, use `copy_for_extend()` to copy the `Value` without incrementing refcount, then call `heap.inc_ref()` separately after the borrow ends.

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
