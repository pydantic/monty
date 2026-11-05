# Separate view origins and adaptive hash caching

These measurements used four inline hash slots.
An intermediate three-slot layout reduced the lazy state from 136 to 128 bytes on the measured 64-bit build.
The current [optional view extension](../lazy-view-cache/README.md) keeps ordinary navigation at 56 bytes and
allocates another 80 bytes only for view caching, promoting the fourth distinct hash to a table.
The measurements below describe the original four-slot combined state.

A separate view-start cursor reduces UTF-8 NDJSON tail parsing from 5.85 ms to
4.32 ms. Promoting the four-entry hash cache to a table reduces repeated hashing
of 64 views from 319 ms to 18.4 ms. Both are close to the Arc control.
Embedding the small hash cache also increases the memory cost of independently
indexed strings: the 100,000-string stress case rises from 47.1 to 56.3 MiB.

## Implementation

The backing string's lazy access state holds two character/byte cursors.
`view_start` anchors the most recently used view; `last` tracks search/index
navigation. Resolving a new view origin preserves the navigation cursor.
Both byte-to-character and character-to-byte lookup can start from the view
origin. Length queries on suffix views reuse that origin and the backing's
cached character count without displacing a search result.

The same lazy state stores four `(start, length, hash)` entries inline.
The fifth distinct range promotes all entries into an `AHashMap`.
Promotion and subsequent table growth use resource-tracker allocation checks;
failed growth preserves existing hashes and can be retried.
Hash hits do not perform growth checks.
The table retains every cached range until the backing string is released,
including ranges whose views have disappeared, as requested for this prototype.
Keys contain two `u32` byte bounds and hold no references to view objects.
Cloning resets the derived access state, and snapshots omit it.

The four small entries avoid the earlier separate hash-cache allocation, but
they enlarge every allocated navigation state, even when no view is hashed.
The state remains lazy: strings that never navigate or hash a view do not
allocate it. Views themselves remain 12-byte payloads inside 16-byte Values.

## CPU time

Five randomized fresh processes per case/build execute repeated-call wrappers.
CPU is macOS `wait4` user+system time divided by the guest call count, including
amortized startup, compilation, fixture loading and teardown. All production
libraries use the optimized `profiling` profile and include #1236's sparse
Unicode index. The previous inline control includes the earlier navigation
fix, with one cursor and a bounded four-entry hash cache.

| Workload                  |      Arc | Previous inline | Dual cursor/adaptive hashes |
| ------------------------- | -------: | --------------: | --------------------------: |
| UTF-8 NDJSON tail parsing |  4.45 ms |         5.85 ms |                     4.32 ms |
| UTF-8 prefix removal      |  4.69 ms |         4.16 ms |                     4.02 ms |
| Hash 4 views repeatedly   |  2.30 ms |         2.47 ms |                     2.50 ms |
| Hash 8 views repeatedly   |  3.37 ms |        41.56 ms |                     3.46 ms |
| Hash 64 views repeatedly  | 17.77 ms |       319.30 ms |                    18.42 ms |
| Split 100,000 ASCII lines |  9.24 ms |         8.45 ms |                     8.47 ms |
| Split 100,000 UTF-8 lines | 10.14 ms |         9.98 ms |                     9.91 ms |

The hash workloads warm 1, 4, 8 or 64 overlapping 8 KiB views of a 16 KiB
ASCII backing and hash each view 2,000 times. The promoted cache avoids the
old cache's cyclic eviction. It still costs a lookup compared with Arc's
per-view cached hash; 8/64-view timings are about 3–4% above Arc in this run.
Small changes in the application workloads should not be treated as isolated
proof of a CPU improvement. Full results and samples are in [results.md](results.md)
and [cpu.csv](cpu.csv).

## Peak RSS

The separate RSS run executes each original fixture once, in five randomized
fresh processes. Numbers are absolute process peaks, including host input
buffers, compilation and interpreter storage; they are not retained heap size.

| Workload                                 |       Arc | Previous inline | Dual cursor/adaptive hashes |
| ---------------------------------------- | --------: | --------------: | --------------------------: |
| Split 100,000 UTF-8 lines                | 52.80 MiB |       41.34 MiB |                   41.31 MiB |
| 100,000 owned Unicode strings, unindexed | 45.38 MiB |       40.83 MiB |                   40.77 MiB |
| 100,000 owned Unicode strings, indexed   | 50.16 MiB |       47.12 MiB |                   56.31 MiB |

The independent-string controls build `str(i) + data` for 100,000 indices,
with `data` equal to 80 `é` characters. The indexed case reads character 20
from every string, activating each owner's navigation state. The unindexed
case retains the same strings without activating those states.
This isolates a cost that shared-backing workloads largely avoid.
The indexed stress case uses about 9.2 MiB more than the previous inline
implementation and 6.2 MiB more than Arc. See [rss.md](rss.md) and [rss.csv](rss.csv).

The short-line splitlines regression remains: 1.61 ms versus Arc's 1.36 ms.
Retaining all promoted hash entries can also accumulate metadata for temporary
views; the memory remains charged to the allocator until backing release.

## Validation and reproduction

The full `memory-model-checks` Monty suite passes: 1,051 tests and doctests.
All 20 Monty string fixtures pass, as do formatting, workspace default/all-feature
clippy and import checks. Tests cover stable navigation across origin lookups,
changing view origins, hash promotion without eviction, allocation failure/retry,
clone reset, source release, snapshots and dictionary lookup after restoration.
All 18 benchmark workloads pass their expected-result checks on all three builds.

`measure.py` uses the local fixtures and executables documented in the parent
experiments. It defines the two independent-string controls directly.
`provenance.json` records executable/input/source hashes and trial counts.
The measured uncommitted source patch is saved locally at
`/private/tmp/monty-dual-benchmark/source.patch`.
