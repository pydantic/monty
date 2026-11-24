# CPython stdlib tests

The sources under `Lib/` come from the CPython release pinned in [`vendor.toml`](../../../monty/stdlib/vendor.toml).
See [LICENSE](LICENSE) and the `unittest` notice in `Lib/unittest/__init__.py`.
The test harness uses these files without adding `unittest` to the sandbox’s bundled modules.

The manifest selects whole files, and the harness runs the patched `unittest/util.py` and `unittest/case.py` before each suite.
It does not execute `unittest/__init__.py`, which imports the discovery and runner machinery.
The [shared compatibility patch](patches/0002-unittest.patch) documents the blockers and keeps failed subtests visible without a `TestResult`.
The [colorsys patch](patches/0001-colorsys.patch) replaces discovery with explicit calls and makes the input generator eager.
The upstream test vectors, assertions, and tolerances are unchanged.

Run the adapted suite on Monty and CPython with `cargo run -p monty-datatest -- test_colorsys.py`.
Use `make vendor-stdlib` to regenerate the sources and `make check-stdlib` to verify them.
