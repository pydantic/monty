# Handler ergonomics: quick performance check

The handler refactor keeps the dispatch gains on N-body and spectral norm.
This is a rough check requested after stopping the longer full-suite measurement, not a precise regression assessment.

## N-body and spectral norm

Median request time in milliseconds, three samples per runtime and mode:

| Workload | Mode | PR #1090 baseline | Previous threaded | Handler macros | CPython |
| --- | --- | ---: | ---: | ---: | ---: |
| N-body | Repeated | 202.634 | 172.617 | 173.565 | 73.393 |
| Spectral norm | Repeated | 154.244 | 119.110 | 119.601 | 69.342 |
| N-body | One-shot | 203.317 | 178.492 | 174.031 | 76.561 |
| Spectral norm | One-shot | 153.837 | 121.166 | 120.297 | 69.861 |

In repeated mode, the macro version takes about **14% less time on N-body** and **22% less on spectral norm** than the baseline.
Its differences from the previous threaded version are +0.55% and +0.41%, respectively.
Those small differences do not establish a performance change with only three samples.
CPython remains faster on both workloads.

These are Flying Circus's `game_nbody.py` and `game_spectral_norm.py` workloads.
The quick run uses one repeated-mode warmup, three timing samples, five startup samples and no RSS measurements.
Compilation, execution and output delivery are timed; worker startup, fixture preparation and session reset are excluded.
Both modes passed output validation for all four runtimes. The quick run did not wait for an isolated machine;
its timings are directional estimates, with no statistically established improvements or regressions.

Host: macOS ARM64. Rust 1.99.0 / LLVM 23.1.1. CPython 3.14.2, tail-call interpreter enabled, JIT disabled.
All Monty runtime binaries use normal release settings: fat LTO, one codegen unit and stripped symbols.
The baseline is PR #1090 head `ca6e09002fbbbad6f85a1b5bdacbd783f62d13c0`.

## Authoring handlers

- Each opcode declares its semantic body and optional fast handler together. The declaration generates both dispatch tables;
  the separate 58-entry fast-handler registration table is gone.
- `direct_handler!` supplies the scalar parameter types and return type.
  `next!`, `next_local!`, `fallback!` and `resume!` supply continuation arguments.
- `with inline` makes the measured subscription-handler inlining policy explicit.

See [Writing threaded opcode handlers](HANDLERS.md) for examples and instruction-pointer/ownership rules.
The macro version's 173 audited handler instruction sequences match the previous unstripped build after normalizing relocated addresses.
172 use indirect tail branches; the subscription handler retains its bounded indirect-call path.

An initial local `Dispatch` object retained fast NOP dispatch but slowed dictionary-heavy loops by about 14%.
That version was rejected. The macros expand directly to the previous scalar operations and restore the dictionary timings.

## Other completed checks

Separate compile-once checks from this follow-up, medians of three rounds with 30 samples per round:

| Workload | Previous threaded | Handler macros | CPython |
| --- | ---: | ---: | ---: |
| NOP, ns/op | 0.505 | 0.505 | 0.437 |
| Empty loop, ms | 1.490 | 1.462 | 1.740 |
| Decrement, ms | 2.133 | 2.100 | 2.952 |
| String-key dictionary, ms | 16.266 | 16.113 | 4.804 |
| Integer-key dictionary, ms | 15.721 | 15.656 | 4.853 |
| Class-member fallback, ms | 8.333 | 8.361 | 2.016 |

NOP uses fitted slopes over streams of 0, 10,000, 100,000 and 1,000,000 instructions.
CPython controls run in each microbenchmark executable, giving six rounds.
The other workload sizes are documented in the [earlier comparison](README.md).
These small differences between threaded versions are observations, not claims of improvements from the macros.

The macro version passes 1,074 core tests (two ignored), all 103 release subprocess tests, and Clippy with warnings denied.
Formatting, import checks and `git diff --check` pass.
The final lint cleanup consolidates `unused_macros` annotations; it does not change executable Rust.
Exact benchmark and final source hashes are recorded in `production-macros/source.json`.

## Artifacts

Under `/private/tmp/dispatch-ergonomics/`:

- `nbody-spectral-quick/`: HTML report, raw JSON, source snapshots and runtime provenance for the two-workload check.
- `production-macros/`: release binaries, build logs, subprocess tests and the exact compiled dispatcher source.
- `macros-development/`: unstripped binaries, assembly and the comparison against all 173 previous handlers.
- `macros-nop/` and `macros-access/`: accepted rounds, raw CSVs, hashes and summaries.
- `macros-tests.log` and `macros-clippy-final.log`: core validation.
- `macros-full-suite-guard/`: preserved partial measurements and contention logs from the interrupted full run.
- `production/`, `nop/` and `access/`: rejected object-based handler experiment.
- `final-source-changes.tgz`: final worktree changes, including the authoring guide.

An earlier three-sample full-suite run remains in `/private/tmp/dispatch-full-suite-initial/`.
It measured the previous threaded build, with 114 runnable workloads, seven unsupported cases and three failures shared with baseline.
The later full-suite run of the macro version was stopped at the user's request; it is not a completed comparison.

Runtime SHA-256:

```text
baseline: bc84f417e544235cfbe33a7f3fd3035ce739cf18308040b76399b1cf09b92f2d
previous: 32469e0dc120697d83ccde5f5f9ac208a2834f3c6af78439f6d94964974233d5
macros:   d5be2770babaae4e59c234b0c16508c33e720e03e3982a5666266157a82f9a53
cpython:  7af94281dcb14ba81b98e587d6e9b237ebf51986f0424bb4988d8568e914e9da
```

The worktree changes remain uncommitted.
