# Monty fixture tests

This runner executes `crates/monty/test_cases/` with Monty and an embedded CPython interpreter.
Use a current Python 3.14 patch release, at least 3.14.8.
Earlier patches lack the float-format precision overflow checks asserted by the fixtures.
The CPython runner rejects an older interpreter before applying fixture expectations, including `xfail`.

PyO3 selects the embedded interpreter at build time.
Changing the Python on your shell's path does not update an already-built runner.
To select it explicitly:

```sh
/path/to/python3.14 --version
PYO3_PYTHON=/path/to/python3.14 cargo run -p monty-datatest
```

Pass a fixture-name filter after `--`, for example `-- percent_format`.
`-- run_test_cases_monty` runs only Monty and does not require the CPython version check to pass.

## Bytearray limitation fixtures

The `bytearray__xfail_*.py` fixtures record CPython behavior for known limitations, including legacy codecs and
the `__getitem__`-only iteration fallback.
These strict `xfail = ['monty']` cases were explicitly requested for PR #1208 as an exception to the repository's
usual no-xfail rule.
They must pass on CPython and fail on Monty; remove the marker when implementing the behavior.
Keep each limitation in its own fixture so one expected failure cannot hide another.
