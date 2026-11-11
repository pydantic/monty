# Interpreter benchmarks

`benches/main.rs` discovers the top-level Python scripts in `benches/python/` at runtime.
Each script checks its result with assertions and registers a Monty benchmark plus a local CPython comparison.
CodSpeed runs the Monty variants.
Scripts compile once before measurement; execution, fixture construction and assertions are timed.

## Unicode strings

The `string_*unicode*` scripts cover:

- Random positive and negative indexing over mixed one-, two-, three- and four-byte characters.
    This uses the same access order as the existing ASCII control, `string_random_access_ascii.py`.
- Nearby reads with small backtracks, adapted from Flying Circus's string-access experiments.
- Initial and repeated length queries across 128 runtime-created strings.
- Successive searches with sliced excerpts, reverse search, counting and a bounded miss.
- The existing first/last-character access workload.

Runtime multiplication and concatenation create fresh heap strings on every execution so derived caches cannot
survive between samples.
These workloads measure Python code in the interpreter, including loop and dispatch costs.

Run the Unicode benchmarks and their CPython comparisons:

```sh
cargo bench -p monty-bench --bench main -- 'string_.*unicode'
```

Execute each once to check results without collecting timings:

```sh
cargo bench --profile dev -p monty-bench --bench main -- 'string_.*unicode' --test
```
