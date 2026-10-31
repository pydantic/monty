# String access benchmarks

These scripts compare existing Monty executables and CPython through Flying Circus.
Use a Flying Circus checkout containing the `agent_unicode_*` workloads and distinct labels for saved builds.
The scripts validate workload outputs and write fixtures, executable hashes, timings, RSS samples and reports to a new
output directory. Generated results are kept outside version control.

```sh
python3 crates/monty-bench/experiments/string-access/run.py \
  --circus /path/to/flying-circus \
  --runtime baseline=/path/to/baseline/monty \
  --runtime candidate=/path/to/candidate/monty \
  --runtime cpython=/path/to/python \
  --output playground/string-access-comparison
```

`run.py` covers ASCII, German, Chinese, 10% Latin-1 and 10% emoji strings.
It measures cold, random, nearby and endpoint reads, plus search operations and retained-string memory.
Memory cases retain 3,000 distinct strings of about 4,096 characters, with 64 accesses per string.
Four agent workloads parse about 1.2 MiB of JSON, filter and rank support records, then produce excerpts and source URLs.
Use repeated `--only` patterns to select workload stems, such as `--only 'memory_index_*'`.

`affix_slices.py` accepts the same runtime, checkout and output arguments.
It measures affix hits, misses, tuples and bounded checks on 16,384-character strings, plus 64-character and whole-string
slices. It also includes the four agent workloads.

Request timing includes compilation and execution of the benchmark body, with fixture preparation excluded.
RSS is a fresh worker's whole-process peak, including preparation, retained values and temporary allocations.
Runs are serial; avoid simultaneous builds or benchmarks.

## Policy and findings

The production cache stores Unicode character length and the last character/byte position.
Only traversals longer than eight characters count toward constructing sparse checkpoints.
Checkpoints are spaced 32 code points apart and built after scanning 32 times the UTF-8 byte length.
Compact offsets cost about 0.125 bytes per character; strings too large for u32 offsets use native-width offsets.
ASCII, endpoints and short local reads avoid checkpoint construction.

Interned ASCII strings store no character-offset cache.
A session allocates a sparse lookup table on its first Unicode cache access, with cache boxes only for accessed
Unicode entries.
A map translates 256-ID groups to append-only cache pages, keeping existing cache references valid during growth.
A late Unicode ID allocates only its own page, rather than empty slots for every preceding ASCII entry.
The last directory lookup is cached so operations on nearby intern IDs avoid hashing.
This keeps ordinary text reads and clones on their original storage path.
Embedding the 24-byte cache in every entry added 6,144 bytes to each 256-slot interner page, including ASCII-only sessions.
The side table is derived state, omitted from snapshots and reset on clone.

A regression test accesses intern ID 1,000,000: the original table filled 1,000,001 slots, while the sparse directory
allocates one 256-slot page and one cache box.
A per-string map regressed repeated Unicode `len()` by 15–16%; memoizing its last lookup still regressed an
alternating-eight-string workload by 12.5%.
Sparse pages reduced that regression to about 1–2% across two runs, while repeated Unicode `len()` improved 3–4%.
Random indexing, nearby indexing, ASCII controls and subtitle search showed no meaningful timing change.
These macOS arm64 comparisons used 50 samples per workload, with a 100-sample check in reversed runtime order
for alternating strings, repeated `len()` and subtitle search.
The remaining alternating-string cost is a tradeoff for eliminating allocation proportional to the highest accessed ID.

A release allocation probe reduced module-clone storage from 16,746 to 10,602 bytes.
Five alternating baseline/candidate runs, each with 30 native timing samples, measured clone medians of 327.5 and 317 ns.
Native keyword calls and session setup varied across runs; these measurements do not establish a fix for their CodSpeed
CPU regressions. A 30-sample Flying Circus comparison measured 100,000 keyword calls at 25.674 versus 24.767 ms per
repeated request, with Unicode random access at 3.513 versus 3.536 ms.
The list-append workload took about 11 ms in both a saved pre-Unicode build and the updated builds on macOS arm64;
CodSpeed's reported simulation improvement did not reproduce as a similar local wall-clock improvement.

A stride of eight was 10–22% faster on sustained random access but used four times the checkpoint storage.
Agent workloads performed similarly with either spacing.
An earlier 16x scan budget built indexes too soon for 64-access strings, causing 15–29% runtime regressions.
The selected policy avoids that allocation, though positive-only 64-access cases retain about 4–7% bookkeeping overhead.

An inline cursor layout removed cursor allocations and runtime borrow checks, saving about 0.35 MiB across 3,000 Unicode
strings. It regressed several access workloads by 2–4%, so the boxed layout remains the default.
The measurements do not isolate the CPU cause of those regressions.

Representative repeated-request medians, measured October 30, 2026 on macOS arm64 with Rust 1.99.0, fat LTO and 30 samples:

| Workload | Saved baseline | Optimized |
| --- | ---: | ---: |
| Mixed Unicode random access, 20,000 lookups | 19.394 ms | 3.371 ms |
| Chinese startswith, 100 calls | 3.103 ms | 0.053 ms |
| Chinese 64-character slices, 1,024 calls | 2.747 ms | 0.517 ms |
| German agent workload | 5.878 ms | 4.513 ms |
| Chinese agent workload | 7.728 ms | 5.898 ms |

These are staged comparisons: random-access and agent baselines precede Unicode caching but already include earlier
ASCII indexing and lowercase changes; affix and slice baselines include the Unicode cache.
The standalone `../unicode-index/` harness compares additional index policies without VM costs.

## Case conversion

`case_conversion.py` compares `lower()`, `upper()` and `casefold()` on 200,000-character ASCII, German, dense Chinese,
mixed Chinese/ASCII and rare-emoji inputs. Each request makes 20 conversions and checks the complete result.
It also runs the standalone German uppercase and casefold workloads in `benches/python/`.

```sh
python3 crates/monty-bench/experiments/string-access/case_conversion.py \
  --circus /path/to/flying-circus \
  --runtime baseline=/path/to/baseline/monty \
  --runtime candidate=/path/to/candidate/monty \
  --runtime cpython=/path/to/python \
  --output playground/case-conversion-comparison
```

Uppercase and casefold use ASCII runs when an aligned ASCII word appears in a bounded 256-byte prefix sample.
Other mixed text retains the original Unicode character loop, which was faster for dense Chinese in preliminary tests.
Missing an ASCII span only selects the fallback; both paths use the same Unicode mappings.
The shared iterator is forced inline to keep decoding inside the conversion loops.
The suite includes lowercase as a control because sharing the iterator can change its code generation even when the
lowercase function itself is unchanged.

Repeated-request medians for 20 conversions of 200,000 characters, measured with 30 samples on macOS arm64,
Rust 1.99.0 release with fat LTO and CPython 3.14.2:

| Workload | Before upper/fold ASCII runs | Guarded, inlined runs | CPython |
| --- | ---: | ---: | ---: |
| German uppercase | 13.221 ms | 3.811 ms | 7.849 ms |
| German casefold | 13.181 ms | 3.804 ms | 13.166 ms |
| Mixed Chinese/ASCII uppercase | 14.920 ms | 9.433 ms | 7.626 ms |
| Mixed Chinese/ASCII casefold | 14.842 ms | 9.477 ms | 12.432 ms |
| Rare-emoji uppercase | 11.250 ms | 0.425 ms | 10.241 ms |
| Rare-emoji casefold | 11.287 ms | 0.436 ms | 13.333 ms |
| Dense Chinese casefold | 15.664 ms | 15.573 ms | 12.526 ms |

The first shared-iterator build regressed dense-Chinese lowercase from 19.583 to 36.412 ms despite unchanged lowercase
source. Keeping the iterator step inlined restored it to 19.363 ms; German lowercase improved from 3.963 to 3.782 ms.
The prefix probe avoids the separate dense-Unicode cost observed when routing every string through ASCII runs.
All benchmark outputs matched the reference; raw reports remain in the local output directories.

## English subtitle search

`../../benches/python/subtitle_search.py` searches original English dialogue for `Alex Rivera` and takes
48-character excerpts, validating positions, `count()` and `rfind()`.
The smaller generated fixture and its limitations are described in `../../benches/python/data/README.md`.

```sh
cargo bench -p monty-bench --bench main -- subtitle_search
```

Earlier measurements used a larger OpenSubtitles corpus and do not apply to this replacement fixture.
