# monty-bench

Criterion benchmarks for [Monty](https://github.com/pydantic/monty), tracked
in CI by [CodSpeed](https://codspeed.io/pydantic/monty). Interpreter
benchmarks run the same workload on Monty and on embedded CPython (via pyo3)
to keep the performance comparison honest; `benches/pool.rs` benchmarks the
subprocess pool (worker spawn, checkout, wire round-trips).

Run from the workspace root:

```console
make bench        # run interpreter benchmarks (release build)
make bench-pool   # subprocess pool benchmarks
make dev-bench    # quick run with the dev profile
make profile      # pprof flamegraphs (unix only)
```

Internal to the workspace; not published.
