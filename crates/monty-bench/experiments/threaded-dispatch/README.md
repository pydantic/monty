# Threaded dispatch experiment

The branch combines bounded threaded dispatch, numeric handlers, borrowed native lookups and fused subscriptions.
It is rebased onto main; [subscription measurements](SUBSCRIPTION_RESULTS.md) describe the local comparison before that rebase.
The dispatcher is always enabled in this worktree; it has no feature switch.

## Historical dispatch-only results

These measurements use PR #1090, `ca6e09002fbbbad6f85a1b5bdacbd783f62d13c0`, as the baseline.
Both builds contain that PR's numeric fast paths, and the candidate changes dispatch and its immediate handlers.
They predate the borrowed and fused subscription changes now included in this branch.

The [handler ergonomics follow-up](HANDLER_RESULTS.md) checks the latest macro-based handlers on N-body and spectral norm.
The measurements below describe the earlier 27-workload comparison.

Measured on macOS ARM64 with Rust 1.99.0 (LLVM 23.1.1) and CPython 3.14.2.
CPython's tail-call interpreter is enabled; its JIT is disabled.
Both Monty runtime binaries use the normal release profile: fat LTO, one codegen unit and stripped symbols.

Across 27 worker workloads: **24 improved, 3 unchanged, 0 regressions**.
The full results and confidence intervals are preserved in the raw summary.

Compile-once core workloads, excluding compilation and IPC (median of three rounds, 30 samples after five warmups per round):

| Workload               | Baseline ms | Threaded ms | CPython ms |
| ---------------------- | ----------: | ----------: | ---------: |
| Empty loop             |       2.502 |       1.443 |      1.706 |
| Decrement              |       4.050 |       2.056 |      2.880 |
| String-key dictionary  |      18.578 |      15.911 |      4.729 |
| Integer-key dictionary |      18.568 |      15.464 |      4.791 |
| Class-member fallback  |       9.195 |       8.151 |      1.976 |

The empty loop runs 200,000 range iterations. Decrement counts down from 200,000.
Each dictionary workload performs 300,000 reads plus arithmetic and loop control; its duration is not an isolated lookup latency.
Class-member fallback performs 100,000 reads of a class attribute through an instance.

NOP dispatch: **1.088 ns/op baseline, 0.507 ns/op threaded, 0.443 ns/op CPython**.

NOP is measured separately with compiled bytecode streams of 0, 10,000, 100,000 and 1,000,000 instructions.
Each executable takes 30 samples per size after three warmups. The result is the median of five independently fitted ns/op slopes.
The examples use the benchmark crate's existing `test-hooks` dependency and `worker-alloc` allocator.
The runtime request comparison uses default features; dispatch itself has no feature switch.

### All worker workloads

| Workload             | Baseline ms | Threaded ms | CPython ms | Change | Status      |
| -------------------- | ----------: | ----------: | ---------: | -----: | ----------- |
| caught_exceptions    |       2.428 |       1.550 |      1.888 | -36.1% | improvement |
| float_arithmetic     |       3.839 |       2.316 |      3.357 | -39.7% | improvement |
| function_calls       |       3.446 |       2.485 |      1.734 | -27.9% | improvement |
| global_loop          |       2.457 |       2.113 |      3.948 | -14.0% | improvement |
| int_comparison       |       7.001 |       3.682 |      3.763 | -47.4% | improvement |
| list_iteration       |       4.564 |       2.769 |      1.544 | -39.3% | improvement |
| list_subscription    |      10.610 |       9.949 |      3.641 |  -6.2% | improvement |
| local_copy           |       2.537 |       1.482 |      1.282 | -41.6% | improvement |
| pair_swap            |      10.854 |       9.853 |      1.228 |  -9.2% | improvement |
| range_descending     |       2.621 |       1.567 |      1.855 | -40.2% | improvement |
| class_fallback       |       9.258 |       8.357 |      2.173 |  -9.7% | improvement |
| decrement            |       4.159 |       2.234 |      3.060 | -46.3% | improvement |
| dict_integer         |      17.808 |      15.616 |      4.975 | -12.3% | improvement |
| dict_string          |      18.270 |      15.885 |      4.898 | -13.1% | improvement |
| empty_loop           |       2.627 |       1.592 |      1.849 | -39.4% | improvement |
| expense_report       |       0.187 |       0.189 |      0.278 |  +1.1% | unchanged   |
| order_cleanup        |       0.230 |       0.227 |      0.305 |  -1.2% | unchanged   |
| api_report           |       0.263 |       0.266 |      0.299 |  +0.8% | unchanged   |
| capacity_planning    |      23.506 |      17.728 |      9.438 | -24.6% | improvement |
| portfolio_risk       |      38.568 |      32.977 |     47.682 | -14.5% | improvement |
| ticket_search        |      19.199 |      18.368 |      6.153 |  -4.3% | improvement |
| game_fannkuch        |    1084.969 |     972.309 |    266.485 | -10.4% | improvement |
| game_nbody           |     203.709 |     172.720 |     73.830 | -15.2% | improvement |
| game_spectral_norm   |     155.417 |     120.741 |     70.002 | -22.3% | improvement |
| perf_float           |     239.139 |     228.606 |     43.897 |  -4.4% | improvement |
| perf_unpack_sequence |      59.792 |      55.446 |     21.981 |  -7.3% | improvement |
| perf_richards        |     183.128 |     176.051 |     26.687 |  -3.9% | improvement |

These are repeated Flying Circus worker requests. Configuration, fixture preparation, output validation and reset are outside the timer;
source compilation, execution and IPC are inside it. Each runtime gets five warmups and 30 accepted samples per workload in shuffled order.
Every output is checked against an independent CPython execution.

The runner checks for competing compilers and benchmark processes before and after each sample, retries detected contention,
and retains rejected attempts. The tables use medians. Classifications use Flying Circus's bootstrap comparison with a 2% relative
and 0.01 ms absolute threshold, and 95% family confidence across baseline and CPython comparisons.
Small positive median differences marked unchanged are not evidence that every execution is faster.
These measurements establish the result for this workload set and machine, not every Python program or architecture.

See [Writing threaded opcode handlers](HANDLERS.md) for the declaration syntax and continuation methods.

## What changed

- All 131 opcode semantics moved into handlers. The common continuation is a small status returned in a register;
    only an exit or uncaught error writes the larger result payload.
- A persistent trampoline retains its context across batches. Direct handlers keep the bytecode slice, instruction pointer
    and instruction countdown in registers. Checkpoint policy stays in the context, so the hot countdown needs no flag mask.
- Local and constant loads copy the active integer, float or interned-string payload directly. Heap-reference loads reserve
    the operand slot before incrementing the reference count, allowing the compiler to exclude a redundant stack-growth path.
- Immediate arithmetic and comparisons use the existing numeric rules directly. Overflow, exceptions and Python callbacks
    retain their ordinary semantic paths.
- Range, list and tuple iteration share small infallible `next_item` helpers. A common `ForIter` followed by `StoreLocal`
    can transfer the owned item directly into an immediate local while accounting for both instructions.

The entry point is [`VM::run`](../../../monty/src/bytecode/vm/mod.rs), with handlers and the trampoline in
[`dispatch_threaded.rs`](../../../monty/src/bytecode/vm/dispatch_threaded.rs).

The counter-only change was measured independently against the preceding threaded build: three clear improvements,
22 unchanged cases, two inconclusive cases and no regressions. Its NOP slopes were effectively unchanged at about 0.51 ns/op.
Earlier controlled comparisons and rejected experiments are retained under the artifact directory below.

## Correctness and code generation

Instruction fetches and stack access remain checked; this experiment adds no unsafe code or bytecode padding.
Handlers publish source state before Python-visible work and reload the active frame after callbacks or frame changes.
Normal chains unwind after at most 256 instructions; nested VM re-entry chains unwind after 16.
Both retain the baseline's first resource check after 255 instructions and subsequent checks every 256 instructions.
Correctness therefore does not require Rust to guarantee tail calls.

The historical dispatch-only build shows indirect tail branches in 172 of 173 emitted opcode-handler functions.
The inlined subscription body retains an indirect call; its stack growth is still bounded by the trampoline.
In that build, the NOP hot path has eight ARM64 instructions, ending in `br`, including its bounds and checkpoint checks.

Validation on the dispatch-only source: 1,074 core tests passed (two ignored), including 14 focused dispatch tests;
all 103 runtime subprocess tests passed in the normal release profile. The tests cover deadlines, worker recovery,
reference ownership, callback re-entry on a 2 MiB stack, caught errors, tracebacks, async tasks, wide locals,
operand-stack growth, iterator mutation and arithmetic boundaries. Clippy passed with warnings denied;
Rust formatting, import checks and the benchmark-tool Ruff checks passed.

## Reproduction and provenance

Use separate Cargo target directories for baseline and candidate. Reusing one target across archived source roots can retain
an executable from the other source tree; the first attempted production comparison caught this through identical hashes
and was discarded before timing.

Build each runtime with the same Python installation and ordinary release settings:

```sh
PYO3_PYTHON=/path/to/python3.14 CARGO_TARGET_DIR=/path/to/separate-target \
  CARGO_INCREMENTAL=0 cargo build --offline --locked --release -p monty-runtime --bin monty
```

Run the request comparison using the Python environment containing Flying Circus's dependencies:

```sh
python tools/measure_workers.py /path/to/new-results \
  baseline=/path/to/baseline/monty threaded=/path/to/candidate/monty \
  --cpython /path/to/python3.14 --flying-circus-source /path/to/flying-circus/src
python tools/summarize_workers.py /path/to/new-results \
  --flying-circus-source /path/to/flying-circus/src --output /path/to/summary
```

The five core workloads and ten opcode-mix workloads are included here. The runner also selects twelve named applications
from the supplied Flying Circus checkout. It copies workload sources and records their hashes, the worker adapter hash,
the harness hash, executable hashes, command line, raw samples and detected process activity.

Build [`opcode_cost`](../../examples/opcode_cost.rs) in both trees with
`cargo build --release -p monty-bench --example opcode_cost --features worker-alloc`, then run:

```sh
python tools/measure_micro.py /path/to/new-nop-results \
  baseline=/path/to/baseline/opcode_cost threaded=/path/to/candidate/opcode_cost --kind nop --rounds 5
python tools/summarize_nop.py /path/to/new-nop-results
```

The baseline source archive includes only the NOP construction hook and benchmark examples in addition to the exact PR head.
[`access_cost`](../../examples/access_cost.rs) provides the compile-once measurements of the five core workloads.
Build it with the same profile, run `tools/measure_micro.py` with `--kind access --rounds 3`,
and summarize with `tools/summarize_access.py`.

Artifacts are under `/private/tmp/dispatch-six-hour/`:

- `production-final-summary/`: the 27-workload table and confidence intervals.
- `production-final-comparison/`: request samples, rejected attempts, workload copies, harness and hashes.
- `production-final-nop/` and `production-final-access/`: accepted rounds, raw CSVs and summaries.
- `production-counter/`: final runtime and microbenchmark binaries, build logs, subprocess tests and source provenance.
- `production-baseline/`: matching baseline binaries and build logs.
- `plain-counter/`: core tests, Clippy, source archive and the unstripped experimental assembly audit.
- `counter-summary/`: the independent counter-only comparison.
- `final-source-changes.tgz` and `final.diff`: final worktree changes.

Runtime SHA-256:

```text
baseline: bc84f417e544235cfbe33a7f3fd3035ce739cf18308040b76399b1cf09b92f2d
threaded: 32469e0dc120697d83ccde5f5f9ac208a2834f3c6af78439f6d94964974233d5
cpython: 7af94281dcb14ba81b98e587d6e9b237ebf51986f0424bb4988d8568e914e9da
```

The earlier PR-head experiment and its initial regressions are preserved in `/private/tmp/threaded-numeric-results/RESULTS.md`.
