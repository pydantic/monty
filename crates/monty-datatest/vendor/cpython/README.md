# CPython stdlib tests

The sources under `Lib/` come from the CPython release pinned in [`vendor.toml`](../../../monty/stdlib/vendor.toml).
See [LICENSE](LICENSE) and the `unittest` notice in `Lib/unittest/__init__.py`.
The test harness uses these files without adding `unittest` to the sandbox’s bundled modules.

The manifest selects the `unittest.TestCase` assertions and helpers used by the `colorsys` suite.
The [test-only patch](patches/0001-colorsys.patch) replaces discovery with explicit calls, makes the input generator eager, and lets the first failed subtest propagate.
The upstream test vectors, assertions, and tolerances are unchanged.

Run the adapted suite on Monty and CPython with `cargo run -p monty-datatest -- test_colorsys.py`.
Use `make vendor-stdlib` to regenerate the sources and `make check-stdlib` to verify them.
