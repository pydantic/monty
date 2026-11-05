# Inline string views: memory savings and cache regressions

The [optional view-cache extension](lazy-view-cache/README.md) removes the per-owner navigation memory regression.

The latest [dual-cursor/adaptive-hash experiment](dual-cursor-hashes/README.md) removes the NDJSON and hash-thrashing regressions.
It also measures the added per-owner cache memory cost.

The later [navigation fix](navigation-fix/README.md) removes the tight Unicode prefix-removal regression.
The measurements below describe the initial prototype before that fix.

The inline `Value::StrView` prototype reduces the peak RSS of splitting 16 MB into
100,000 lines by about 11.5 MiB compared with the published `Arc<String>` views.
It also regresses repeated hashing and Unicode prefix removal. These results do
not support replacing the Arc implementation with this prototype as it stands.

All three builds include #1236's UTF-8 indexing work:

- Copy: `5c9dddf39c92981b8ef023be059e8c3aeabf7e91`.
- Arc views: `f29350439dc1a5d5f63e7c0d30555edcf170a57c`.
- Inline views: uncommitted prototype on the Arc revision. It uses three `u32`
    fields in a 16-byte `Value`, holds a heap reference to the original string,
    shares the backing's Unicode access cache, and uses a lazy four-entry hash
    cache keyed by byte start and length.

Ordinary views retain the 16-byte minimum and 25% backing ratio. Splitlines
continues to allow every non-inline result to share, regardless of ratio.

## Results

The table below reports the initial five-process medians. Full results for all
21 cases are in [results.md](results.md). CPU time is per guest workload call;
RSS is the absolute peak of a separate process executing the workload once.

| Workload                    |   Arc CPU | Inline CPU | Arc peak RSS | Inline peak RSS |
| --------------------------- | --------: | ---------: | -----------: | --------------: |
| JSON request                |  2.040 ms |   1.956 ms |    15.48 MiB |       15.33 MiB |
| NDJSON UTF-8 prefix removal |  4.436 ms |   6.223 ms |    12.25 MiB |       12.27 MiB |
| Split 100,000 ASCII lines   |  9.229 ms |   8.455 ms |    52.80 MiB |       41.34 MiB |
| Split 100,000 UTF-8 lines   | 10.073 ms |   9.892 ms |    52.83 MiB |       41.38 MiB |
| Split 100,000 inline lines  |  1.358 ms |   1.610 ms |    11.09 MiB |       11.11 MiB |
| Hash 4 views repeatedly     |  2.331 ms |   2.453 ms |     8.50 MiB |        8.55 MiB |
| Hash 8 views repeatedly     |  3.337 ms |  41.631 ms |     8.52 MiB |        8.55 MiB |
| Hash 64 views repeatedly    | 17.729 ms | 320.333 ms |     8.52 MiB |        8.55 MiB |
| Remove ASCII prefixes       |  3.985 ms |   3.799 ms |     8.30 MiB |        8.30 MiB |
| Remove UTF-8 prefixes       |  4.711 ms |  19.550 ms |     8.42 MiB |        8.42 MiB |

A second randomized run with seven processes per case confirmed the regressions:
8-view hashing was 12.38x slower, 64-view hashing 17.94x slower, UTF-8 prefix
removal 4.12x slower, NDJSON UTF-8 prefix removal 1.37x slower, and splitting
inline lines 1.19x slower. ASCII bulk splitting remained about 9% faster.
See [confirmation.log](confirmation.log) and [confirmation.csv](confirmation.csv).

The hash stress cases create 1, 4, 8 or 64 overlapping 8 KiB views of a 16 KiB
ASCII input, warm their hashes, then hash each view 2,000 times. Cycling through
more than four ranges evicts every cached entry before its next use. The Arc
implementation retains each view's hash separately. This test deliberately
exposes the bounded cache's worst case; it is not an application workload.

Unicode prefix removal starts with 20,000 characters (`é🌍` repeated 10,000
times). Sharing the backing's navigation cache does not reproduce the Arc
implementation's cached per-view character length at the same cost. Further
profiling is needed before attributing the entire regression to one operation.
Template-style parsing and DOCX text extraction change by less than 2% in this
run. Those fixtures simulate extraction patterns; they do not run Jinja2 or a
DOCX library inside Monty.

## Measurement and reproduction

`measure.py` generates the additional hash/prefix cases and repeated-call wrappers,
then runs the three production-library integration executables. The optimized
`profiling` build omits test-only string allocation counters. The executable
hashes match the earlier copy and Arc experiments; provenance records all three
hashes, input hashes, revisions and the measured source patch hash.
The local source patch is `/private/tmp/monty-inline-benchmark/measured-source.patch`.

The initial run randomizes five fresh processes per case/build. It measures
per-child macOS `wait4` user+system CPU, divided by the guest repetition count.
Calls share a VM and its JSON cache. Startup, compilation, fixture loading and
teardown are amortized into that time. The separate RSS run executes the
original fixture once per process, also five times. RSS includes host-side
input buffers, compilation and interpreter storage; it is not retained heap
usage after collection. MiB means 2^20 bytes.

The saved scripts use local `/private/tmp` paths. Input data for the existing
15 scenarios is in `/private/tmp/monty-splitlines-fixtures`; the original source
programs (`*.py.txt`) and manifest are saved in `workloads/`. Generated hash/prefix inputs
are defined directly in `measure.py`. `rss-harness.rs` is installed temporarily
as `crates/monty/tests/peak_rss.rs` and built with
`cargo test -p monty --profile profiling --test peak_rss --no-run`.
Copy each resulting executable to its distinct path before building another
worktree with the shared Cargo target directory. `confirm.py` reruns seven
processes for the cases above.

All 21 original scenarios pass their expected-result checks on CPython and all
three Monty builds. These benchmark checks do not replace the full semantic,
snapshot and resource-limit suites for the unfinished inline implementation.
The five focused string-view tests also pass with `memory-model-checks`, including
snapshot restoration after releasing the source and a live iterator.

Allocation counts and post-release retained memory were not measured in this
run. The splitlines ratio exception still allows one small line to retain the
entire backing allocation.
