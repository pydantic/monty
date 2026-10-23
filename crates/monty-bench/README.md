# Benchmarks

The CodSpeed workflow measures wall time and memory allocations.
Normal pull requests run the six Rust-defined interpreter benchmarks: end-to-end execution, parsing, module cloning,
REPL feeds after many snippets, session dumps and session loads.
Scripts with `[bench] always = true` also run in the PR suite; unmarked scripts run only in the full suite.
Nine scripts are marked, covering general execution, calls, attribute access, list allocation, dict comprehensions,
JSON parsing and serialization, cycle collection, and order-data aggregation.
Together with the Rust-defined benchmarks, the PR suite runs 15 benchmarks.

```python
# /// monty
# [bench]
# always = true
# ///
```

Reduced runs build only the `main` target and skip the release worker used by pool benchmarks.

The full suite runs on pushes to `main`, manual workflow runs, and pull requests with the `Full Benchmarks` label.
Adding that label starts a full run immediately; further pushes keep running the full suite while the label is present.
Removing it restores reduced runs on subsequent pushes.
Other label changes do not start benchmark jobs.
Full runs include all benchmark targets and build the release worker.
They also maintain the main-branch baselines for the reduced suite, whose benchmark names and build settings are unchanged.

To run the reduced suite locally:

```bash
cargo codspeed build -p monty-bench --profile codspeed --features codspeed --bench main
MONTY_BENCH_SUITE=pr cargo codspeed run -p monty-bench --bench main
```

CI selects the measurement mode with `cargo codspeed build -m walltime` or `-m memory`.

`MONTY_BENCH_SUITE` accepts `pr` or `full`; leaving it unset runs the full suite.
The workflow selects the suite without changing benchmark names.
