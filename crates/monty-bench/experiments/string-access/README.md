# Unicode access caches

This change follows the scan fixes. Positive and negative Unicode indexing and explicit character bounds
can still repeatedly traverse prefixes after that first change.
Each accessed string now caches its character length and last character/byte position.
Sustained distant access builds sparse byte-offset checkpoints after scanning 32 times the UTF-8 byte length.
Traversals of eight characters or fewer do not count toward that budget.

| UTF-8 byte length | Offset width | Checkpoint spacing |
| ----------------- | ------------ | ------------------ |
| Up to 255         | u8           | 8 characters       |
| Up to 65,535      | u16          | 16 characters      |
| Up to u32::MAX    | u32          | 32 characters      |
| Larger            | usize        | 32 characters      |

The first three policies use about one bit per character for the checkpoint buffer, plus rounding.
On a 64-bit build the StringIndex metadata occupies 24 bytes and its lazy boxed cursor state 48 bytes.
ASCII and endpoint reads avoid that state. Checkpoint allocations use the resource tracker.
Derived caches reset on clone and are omitted from snapshots.

Interned Unicode strings allocate caches in sparse 256-ID pages outside the text table.
Only accessed pages and strings allocate storage; a late ID does not reserve all preceding entries.
A last-page lookup memo avoids repeatedly hashing nearby IDs.
Existing entries retain stable addresses while runtime compilation adds strings.

`run.py` compares ASCII, German, Chinese, 10% Latin-1 and 10% emoji workloads, including cold,
random and nearby indexing, search and retained strings. `affix_slices.py` adds bounded affix and slice cases.
Use distinct workers for main, scan fixes and caches; do not attribute the first PR's gains to the second.

```sh
python3 crates/monty-bench/experiments/string-access/run.py \
  --circus /path/to/flying-circus \
  --runtime baseline=/path/to/scan-fixes/monty \
  --runtime candidate=/path/to/caches/monty \
  --runtime cpython=/path/to/python \
  --output playground/cache-comparison
```

Fixture setup is outside request timing. RSS is a separate fresh-worker whole-process peak,
including setup and temporary allocations. Run serially without simultaneous builds.
The original English dialogue fixture in `benches/python/data/` exercises repeated search and short excerpts.
The benchmark checks match counts, excerpt checksums and the final match position.
Raw experiment outputs are excluded from version control.

Earlier policy experiments compared u32 checkpoints every 32 characters with adaptive widths and spacing.
On macOS arm64, 50-sample full-interpreter comparisons showed 6–16% faster random access below 64 KiB,
and about 8% faster access across 1,000 heavily indexed strings, with essentially unchanged RSS.
Those figures compare cache policies, not this PR against the cache-free scan fixes.
A 100-sample reversed-order search check measured repeated searches within ±0.7%; some short one-shot
searches were 1–2.4% slower. Finer checkpoints add work to inverse-offset binary searches.
