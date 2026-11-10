# Borrowed and fused subscriptions

Native list, tuple and dict hits borrow operands and acquire result ownership before releasing inputs.
Custom keys, misses, invalid indices and Python callbacks retain ordinary protocol dispatch.
Fused local, literal-key and global loads avoid intermediate owners and can store integer results directly.
The new opcodes take dump version 16; compatible older dumps remain readable.

Handlers keep bytecode and instruction offsets in scalar arguments until fallback or a checkpoint.
Temporary literal keys live in an outlined read-only helper, so their addresses do not block the dispatcher's tail continuation.
Folded key loads and stores consume their original instruction budget; a boundary keeps the remaining instructions pending.

## Local measurements before the rebase

All Monty builds used main `7c13f105`, Rust 1.99.0 / LLVM 23.1.1, fat LTO, one codegen unit and no PGO.
The threaded contribution was PR #1296 at `f67c00f0`, applied to that same base.
CPython 3.14.2 used PGO/LTO, its tail-call interpreter and no JIT.
This comparison includes #1090 in the threaded builds, but not in the plain baseline or subscription-only build.
It therefore measures the combined numeric/dispatch contribution, rather than dispatch alone.
These numbers do not measure the later rebase onto main.

Median ns/access from eight randomized rounds, with independent processes, a time-based warmup and twelve retained batches:

| Access                     | Baseline | Threaded | Subscriptions | Combined | CPython |
| -------------------------- | -------: | -------: | ------------: | -------: | ------: |
| Local list, variable index |    32.95 |    30.36 |          6.82 |     6.01 |    4.95 |
| Local dict, variable key   |    47.45 |    54.05 |         13.26 |    12.19 |   10.00 |
| Global list, variable key  |    33.25 |    37.15 |         10.39 |     9.31 |   10.01 |
| Global dict, variable key  |    47.20 |    50.49 |         14.08 |    13.75 |   13.23 |
| Local dict, literal key    |    46.68 |    44.41 |         13.98 |    13.46 |   11.10 |
| Local list, literal index  |    32.23 |    30.82 |          9.93 |     8.26 |    6.15 |

The first integration regressed global and literal-key accesses against subscription fusion alone.
Keeping offsets in scalar arguments and isolating temporary keys removes those regressions in this sample set.
Both affected handlers emit tail continuations in the measured ARM64 binary.

A full Flying Circus `--quick` screen covered 124 workloads in one-shot and repeated modes.
Every Monty configuration measured 114, marked seven unsupported and failed the same three cases; CPython measured all 124.
Pickle/unpickle return unhandled OS-call events, and regex-effbot requires unsupported function attributes.
Across the 114 shared successes, the combined build's repeated-mode geometric mean takes 9% less time than baseline,
4.5% less than threaded alone, and 25% more than CPython.
The three-sample screen has wide ranges on short requests and does not establish small performance differences.

## CI coverage

The `subscript_*_read.py` benchmarks register automatically in the interpreter benchmark suite.
They cover local variable keys, literal keys, globals and rotating heap results.
CodSpeed runs simulation and memory by default; the PR's `Full Codspeed` label also enables walltime.

Ownership, snapshot/resume, traceback, aliasing, wide-slot and timeout checks cover generic fallback and native hits.
Changing results across ordinary checkpoints and `__repr__` reentry catch skipped stores and instruction-budget mistakes.
No heap-module or unsafe-code changes are required.
