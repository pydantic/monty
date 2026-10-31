# Unicode index policy experiments

This standalone Rust harness compares lookup policies without VM dispatch, Python argument conversion or heap ownership
costs. Its scan baseline copies the lookup algorithm used before the Unicode cache.
Results describe algorithm costs rather than predicted interpreter speedups.

```sh
python3 crates/monty-bench/experiments/unicode-index/run.py --output playground/unicode-index-results
```

The script builds with `rustc --edition=2024 -O`, validates every policy, runs the timing matrix and measures RSS.
Only Rust and the Python standard library are required.
RSS collection supports Linux and macOS; use `--rss-samples 0` to skip it or `--skip-matrix` for memory measurements only.
A new output directory receives generated timing CSVs, memory samples and build provenance; these are not versioned.

## Matrix

The 22 policies cover cached length, cursors, checkpoints every 8/32/128 code points, full indexes and non-ASCII deltas.
Triggers use either a fixed access count or accumulated scanning of 1/4/16/64 times the text's byte length.
Traversals of at most eight characters do not count toward adaptive construction.
ASCII strings and first/last-character access bypass indexes.

Inputs contain 256, 4,096 or 65,536 characters: ASCII, German prose, 10% Latin-1, Chinese prose or 10% emoji.
Each cache lifetime makes 1/16/256/4,096 reads using random, sequential, reverse, nearby, repeated or endpoint patterns.
Half the requests use negative indexes.
Timing includes cache construction and cleanup while excluding input generation.
Each entry is the median of seven samples with policy order rotated between samples.
Memory runs retain 3,000 separate 4,096-character strings in fresh processes.

## Findings

On the October 30, 2026 macOS arm64 run, cached length reduced negative-index work, while a cursor helped nearby reads.
For 4,096-character German strings and 4,096 random reads, the scan baseline took 171 ns/read, stride eight 16 ns, and
stride 32 about 34 ns. The checkpoint buffers cost 2,048 and 512 bytes per string respectively.
A full index reached about 9 ns/read but retained 16,384 bytes per string.

Cold access reversed the tradeoff: for 16 German random reads, the scan took 137 ns/read and fixed stride-eight
construction took 206 ns/read. Deferring construction based on scanning cost avoided that early allocation.
Non-ASCII delta tables were compact for sparse Unicode but larger than a full index for dense Chinese.

All policies share a 128-byte prototype state, including unused fields, so this harness isolates index-buffer costs.
Its u32 offsets are valid for its bounded inputs; production needs wider offsets for larger strings.
Choose defaults using the VM benchmarks in `../string-access/`, which measure VM timing, RSS and application workloads.
