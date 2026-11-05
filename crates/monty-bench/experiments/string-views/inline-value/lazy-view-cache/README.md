# Allocate view caches only for views

Moving view-only metadata out of the ordinary Unicode navigation cache reduces
peak RSS for 100,000 independently indexed Unicode strings from 56.31 to
47.14 MiB. Indexed execution changes from 49.95 to 47.86 ms in this run.
Unindexed strings remain at 40.80 MiB. The chunker, NDJSON, prefix-removal and
bulk splitlines results remain close to the published combined-cache version.

## Layout

The string header still caches its whole-string hash, character count and ASCII
status. Ordinary Unicode navigation creates a `NavigationCache` containing one
cursor, the scan budget, sparse offsets and an optional view-extension pointer.
The extension is allocated only when a view needs its origin anchor or hashes.

Measured 64-bit Rust sizes:

| Storage                                     | Bytes |
| ------------------------------------------- | ----: |
| Owning `Str` header, unchanged              |    56 |
| `AccessCache` inside that header, unchanged |    24 |
| Ordinary `NavigationCache` allocation       |    56 |
| Optional `ViewCache` allocation             |    80 |
| Navigation and view allocations together    |   136 |

Unused navigation/view state still costs no allocation. The published control
has one 136-byte state with four inline hashes. The intermediate three-hash
combined state was 128 bytes. This implementation keeps three inline hashes,
promotes on the fourth, and retains every cached range until backing release.
The extra view allocation trades eight bytes of raw storage versus the
three-slot combined layout for 72 bytes less ordinary navigation storage.
Allocator overhead, table buckets and sparse-index buffers are additional.

Creating the extension preserves the existing length count, cursor and sparse
index in place. Unicode views retain their separate origin/navigation cursors.
Hash-table growth remains resource checked. Clones reset the access cache;
snapshots omit it. String values, heap ownership and snapshot formats are unchanged.

## CPU time

Five randomized fresh processes per case/build execute repeated guest calls.
Values are median per-call macOS `wait4` user+system CPU, including amortized
startup, loading, compilation and teardown. All use production libraries in the
optimized `profiling` profile. The Arc control is `f2935043`; the published
four-slot combined inline layout is `61728c1c`; the optional extension is an
uncommitted change on that commit.

| Workload                         | Published combined inline-4 | Optional view-3 |
| -------------------------------- | --------------------------: | --------------: |
| 100,000 unindexed Unicode owners |                   23.440 ms |       23.434 ms |
| 100,000 indexed Unicode owners   |                   49.950 ms |       47.860 ms |
| NDJSON UTF-8 tail parsing        |                    4.424 ms |        4.448 ms |
| UTF-8 prefix removal             |                    3.995 ms |        3.990 ms |
| Hash 64 views repeatedly         |                   18.532 ms |       18.627 ms |
| Split 100,000 UTF-8 lines        |                    9.893 ms |        9.927 ms |
| 2 MiB remainder chunker          |                    1.699 ms |        1.723 ms |
| 2 MiB offset chunker             |                    1.475 ms |        1.471 ms |

The initial short single-view hash batch was about 7% slower. Longer batches
amortizing startup reduce that difference to about 2%: 0.554 to 0.565 ms.
Four-view hashing changes from 1.533 to 1.437 ms, about 6% faster; it now uses
the promoted table. See [hash-confirmation.csv](hash-confirmation.csv).
Small timing differences remain local results, not proof of an improvement
across machines. Full controls are in [results.md](results.md) and [cpu.csv](cpu.csv).

## Peak RSS

Separate fresh processes execute each original workload once, five times per
case/build. These are median absolute process peaks in MiB, including runtime,
harness, input buffers, compilation and live guest storage.

| Workload                         |       Arc | Published combined inline-4 | Optional view-3 |
| -------------------------------- | --------: | --------------------------: | --------------: |
| 100,000 unindexed Unicode owners | 45.38 MiB |                   40.80 MiB |       40.80 MiB |
| 100,000 indexed Unicode owners   | 50.16 MiB |                   56.31 MiB |       47.14 MiB |
| Split 100,000 UTF-8 lines        | 52.84 MiB |                   41.33 MiB |       41.38 MiB |
| 2 MiB remainder chunker          | 16.20 MiB |                   16.22 MiB |       16.31 MiB |

Thus the owner-indexing regression is removed without a comparable change for
one-backing workloads. See [rss.md](rss.md) and [rss.csv](rss.csv).
These peaks are not post-collection retained-memory measurements.

## Validation and reproduction

The full Monty suite with `memory-model-checks ref-count-return` passes 1,079
tests/doctests. All 20 Monty string fixtures pass. Focused tests verify that
ordinary indexing leaves the view extension absent, adding it preserves the
existing allocation/cursor/index/length count, and populated view caches do
not change dump bytes. Existing tests cover hash growth failure/retry,
source release, snapshots, view ownership and collection of cycles containing views.
All 105 subprocess tests also pass, including allocator-backed memory limits.
Rust formatting, workspace default/all-feature clippy and import checks pass.

`measure.py` reuses the fixtures prepared by the earlier
[dual-cursor/hash comparison](../dual-cursor-hashes/README.md) and
[agent chunker](../../agent-chunking/README.md). The chunker uses its existing
calibrated optimized wrapper for both old and new builds, keeping its repeat
count identical. `confirm_hashes.py` prepares longer wrappers for the two short
hash cases. Every measured process checks its expected result.

```sh
python measure.py --new-exe /path/to/new-harness
python confirm_hashes.py
```

`measure.py` accepts fixture-directory overrides; the confirmation script uses the local directories from those parent reports.
Build the production integration executable with the shared
[RSS harness](../../splitlines/rss-harness.rs), and freeze each executable before
building another worktree with the same Cargo target directory.
`provenance.json` records executable/input/source hashes, revisions and counts.
The source patch is saved locally at `/private/tmp/monty-lazy-view-cache/source.patch`.
