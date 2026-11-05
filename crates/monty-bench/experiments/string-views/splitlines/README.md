# Splitlines views without the retention ratio

This experiment lifts the 25% retention rule for splitlines only.
Every result above the 15-byte inline limit shares heap-backed storage, composing offsets directly into the original backing.
Interned strings still materialize results, and ordinary slicing retains its existing 16-byte / 25% policy.
The prototype compares both the published `Arc<str>` backing and the `Arc<String>` + shrink_to_fit trial.
All variants include #1236's Unicode cursor/sparse-index code and use the same fixtures.

The changes are limited to string storage, splitlines dispatch and tests; no heap-module or GC changes are required.
A DropGuard releases partial line lists if a memory/time checkpoint raises during splitting.

## Throughput

These are new repeated-session measurements, not the earlier fresh-VM latency numbers.
A production-library integration executable runs a guest function repeatedly in one VM: 100 calls for NDJSON/inline
cases and 10 for the 100,000-line cases.
Five fresh processes per workload/build execute the same generated loop; the table reports median per-call user+system
CPU time from macOS wait4(2), including amortized startup, compilation, input loading and teardown.
The original input remains available to the caller, and repeated JSON calls share that VM's string cache.
All four production libraries omit the test-only string counter hooks.

| Workload                | `Arc<str>` copy | `Arc<str>` views | `Arc<String>` copy | `Arc<String>` views |
| ----------------------- | --------------: | ---------------: | -----------------: | ------------------: |
| ndjson_ascii_splitlines |        3.507 ms |         3.363 ms |           3.575 ms |            3.397 ms |
| ndjson_utf8_splitlines  |        2.704 ms |         2.608 ms |           2.755 ms |            2.593 ms |
| splitlines_bulk_ascii   |       12.719 ms |         9.235 ms |          14.131 ms |            9.117 ms |
| splitlines_bulk_utf8    |       13.625 ms |        10.220 ms |          15.055 ms |           10.072 ms |
| splitlines_keep_one     |       13.775 ms |        10.251 ms |          15.107 ms |           10.065 ms |
| splitlines_inline       |        1.094 ms |         1.288 ms |           1.116 ms |            1.283 ms |

The two bulk cases each contain 100,000 lines in 16,000,000 UTF-8 bytes (15.26 MiB).
Each content line occupies 159 bytes; UTF-8 lines contain 80 characters.
The NDJSON fixtures are the same roughly 1 MB request/event data used in the earlier report.
The inline control contains 100,000 five-character lines, which continue to use inline values.

Views improve bulk splitting/cleanup by 25–27% with `Arc<str>` and 33–35% with `Arc<String>`.
NDJSON processing improves about 4–6%; decoding/aggregation still accounts for most of its runtime.
The inline control regresses about 15–18% (roughly 0.17–0.19 ms per 100,000 lines), without an RSS benefit.
That regression is a cost of this prototype, not evidence that short lines need views.

## Peak RSS

Each process executes the original workload once, separately from the throughput loop.
Five fresh processes per workload/build are randomized; cells are median absolute process peaks in MiB (2^20 bytes).
Peaks include fixture loading, host input cloning/import, guest compilation, execution and result checking.
The empty-harness footprint is not subtracted.
Retained fixtures keep the REPL alive through their result check.

| Workload                | `Arc<str>` copy | `Arc<str>` views | `Arc<String>` copy | `Arc<String>` views |
| ----------------------- | --------------: | ---------------: | -----------------: | ------------------: |
| empty                   |        6.28 MiB |         6.25 MiB |           6.22 MiB |            6.23 MiB |
| ndjson_ascii_splitlines |       13.94 MiB |        13.31 MiB |          13.05 MiB |           12.31 MiB |
| ndjson_utf8_splitlines  |       13.53 MiB |        12.45 MiB |          12.61 MiB |           12.12 MiB |
| splitlines_bulk_ascii   |       86.41 MiB |        68.09 MiB |          72.61 MiB |           52.81 MiB |
| splitlines_bulk_utf8    |       86.42 MiB |        68.08 MiB |          72.61 MiB |           52.81 MiB |
| splitlines_keep_one     |       86.14 MiB |        68.05 MiB |          72.81 MiB |           52.75 MiB |
| splitlines_inline       |       11.67 MiB |        11.67 MiB |          11.11 MiB |           11.08 MiB |

Bulk-text peaks fall by about 18.3 MiB with `Arc<str>` and 19.8 MiB with `Arc<String>`.
For NDJSON the reductions are about 0.5–1.1 MiB.
The large-input results show a stronger benefit than the earlier small-request representation-only comparisons.

## Retaining one line

The keep-one fixture splits the 16 MB UTF-8 input, retains only line 123, and releases the Python input and other lines.
The retained line is 80 characters / 159 bytes.
Separate test-only backing counters compare copy and view policies within the `Arc<str>` implementation:

| State                                                   | Copy lines | View lines |
| ------------------------------------------------------- | ---------: | ---------: |
| One surviving line: live backing bytes                  |        159 | 16,000,000 |
| All lines surviving: live backing bytes                 | 15,900,000 | 16,000,000 |
| Bulk split: string-backing allocations including source |    100,001 |          1 |
| Bytes copied into line payloads                         | 15,900,000 |          0 |

Thus a single surviving line can retain about 100,629 times its visible byte length.
The existing 4x backing/view bound deliberately does not apply to splitlines results in this experiment.
Peak RSS alone does not expose this retention regression: copying can have a higher temporary peak while leaving
much less live backing storage after the other lines disappear.
Backing counters exclude VM objects, host input storage, Arc headers and allocator overhead.

## Validation and reproduction

The `Arc<str>` memory-model suite passes 1,045 interpreter/doc tests, including the new splitline-view case.
The companion `Arc<String>` suite passes 1,047 interpreter/doc tests.
The string fixtures cover all Python line terminators, CRLF, keepends, empty/inline lines and Unicode strings.
The new test also splits a parent view, releases the parent/source, and checks the surviving line's contents, indexing and hash.
All 20 Monty string fixtures and independent CPython checks of all 15 generated scenarios pass.
Both worktrees pass Rust formatting and workspace clippy/import checks.

The arcstr-views.patch file reconstructs the Arc<str> view variant from f7be4e56 using `git apply --unidiff-zero`.
Commit 91380f0f records the Arc<String> copy variant; the current implementation uses Arc<String> views.
Generate fixtures with MONTY_SCENARIO_EXPORT_DIR and the ignored benchmark_string_realistic_scenarios test in a debug build.
The debug harness emits backing metrics without timing loops; its explicit copy policy also disables splitlines views.
Copy rss-harness.rs into crates/monty/tests/peak_rss.rs and build it with the profiling profile for each variant.
measure.py records the process measurements using those four executables; its paths identify this run's local artifacts.
The backing-metrics CSV contains all 90 scenario/policy rows, while cpu-samples.csv and rss-samples.csv contain the raw process samples.
provenance.json records executable/input hashes, loop counts and trial source hashes.
