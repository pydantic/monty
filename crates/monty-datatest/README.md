# monty-datatest

The data-driven test harness for [Monty](https://github.com/pydantic/monty):
it runs every Python file in `crates/monty/test_cases/` against **both Monty
and CPython** (embedded via pyo3) and compares the results — return values,
printed output, tracebacks, and reference counts — so any behavioural
divergence between the two interpreters fails the suite.

Run via `make test-cases` from the workspace root, or directly:

```console
cargo run -p monty-datatest             # all test cases
cargo run -p monty-datatest str__ops    # only cases matching a filter
```

`make complete-tests` fills in incomplete test expectations using CPython's
actual output. See the "Tests" section of
[`CLAUDE.md`](https://github.com/pydantic/monty/blob/main/CLAUDE.md) for the
test-case file conventions (assert style, `TRACEBACK:` blocks, expectation
comments, fixture markers).

Internal to the workspace; not published.
