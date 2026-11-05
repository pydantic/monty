# Unicode navigation fix for inline views

Unicode prefix removal improves from 19.51 ms to 4.17 ms after removing redundant
length conversions and preserving local cursor access. The Arc control takes
4.72 ms in the same run. The fixed inline implementation remains 33% slower
than Arc in the NDJSON UTF-8 tail parser.

The fix changes contiguous slice boundary conversion:

- Normalized boundaries reuse the character length already calculated for the slice.
- Boundaries within eight characters of either visible end are read locally.
- Nearby backward byte-to-character lookups reuse the cursor instead of restarting at a checkpoint.
- Scan work accumulated by infallible view-length queries triggers checkpoint construction at the next fallible offset
    lookup or contiguous view slice, including endpoint lookups that would otherwise return immediately.
    Construction still checks the allocation budget and deadline, and failure leaves the cache able to retry.

The inline payload and string ownership are unchanged. The fix adds no per-view
cache or allocation. The four-entry hash-cache policy is unchanged.

## Measurements

Seven randomized fresh processes per workload/build execute the same repeated-call
fixtures used by the initial inline-view experiment. The suffix-removal fixtures
use the same 20,000-character ASCII and UTF-8 inputs as prefix removal, with
`s = s[:-1]` instead of `s = s[1:]`. CPU is per-call user+system time from macOS
`wait4`, including amortized startup, compilation, input loading and teardown.
All builds use the production library and optimized `profiling` profile.

| Workload                  |      Arc | Inline before | Inline fixed |
| ------------------------- | -------: | ------------: | -----------: |
| ASCII prefix removal      |  4.00 ms |       3.84 ms |      3.74 ms |
| UTF-8 prefix removal      |  4.72 ms |      19.51 ms |      4.17 ms |
| ASCII suffix removal      |  4.10 ms |       3.84 ms |      3.82 ms |
| UTF-8 suffix removal      | 17.42 ms |       4.54 ms |      4.39 ms |
| NDJSON UTF-8 tail parser  |  4.41 ms |       6.20 ms |      5.86 ms |
| Split 100,000 ASCII lines |  9.13 ms |       8.39 ms |      8.30 ms |
| Split 100,000 UTF-8 lines | 10.09 ms |       9.91 ms |      9.83 ms |

The separate single-execution RSS run uses five fresh processes per case/build.
Bulk splitting peaks at 52.83 MiB with Arc and 41.36 MiB with the fixed inline
implementation for both ASCII and UTF-8. Prefix-removal peaks remain about
8.4 MiB. These are absolute process peaks, including host input buffers and
compilation, not retained heap size after collection.

All 13 workloads and raw samples are in [results.md](results.md), [cpu.csv](cpu.csv)
and [rss.csv](rss.csv). `measure.py` uses the local fixture directories and
executables documented by the parent experiment. Executable hashes, input hashes,
trial counts and the source patch hash are in [provenance.json](provenance.json).
The local source patch is `/private/tmp/monty-inline-navigation-benchmark/current-source.patch`.

## Validation

The full Monty suite passes with `memory-model-checks`: 1,047 tests and doctests passed.
All 20 Monty string fixtures pass.
This includes ten cache tests and six focused string-view tests.
New regressions cover nearby backward cursor traversal, index construction after
range-length scans, allocation failure/retry, and repeated trims of 0–8 Unicode
characters from both ends. Existing tests cover adaptive checkpoint thresholds,
index widths, random lookups, deadlines, snapshots and source/view lifetime.
Workspace default and all-feature clippy/import checks pass via `make lint-rs`.

The earlier hash-cache thrashing and short-line splitlines regression remain
outside this navigation fix.
