# Overlapping UTF-8 window strides

Large overlapping windows improve by 7–10x over the UTF-8 index PR alone, and
114–195x over current main, in this deliberately favorable slicing kernel.
The [agent document chunker](../agent-chunking/README.md) exercises a real
paragraph-aware policy and also compares against CPython.

## Workload

Each input contains 131,072 Unicode characters: predominantly two-byte Latin/
Greek, predominantly three-byte CJK, or mixed one-/two-/three-/four-byte text.
ASCII sequence identifiers make the generated document nonperiodic.

The main kernel creates 2,048 overlapping 65,536-character windows:

```python
for i in range(2048):
    start = (i * stride) % (len(data) - width + 1)
    window = data[start:start + width]
    total += len(window) + ord(window[0]) + ord(window[width // 2]) + ord(window[-1])
```

Strides 1, 8, 64 and 1,024 advance the contiguous window in Python character
positions, wrapping through the valid starting positions. They are not the
`step` in `s[start:stop:step]`. Large windows cover about half the backing and
qualify for the ordinary 16-byte / 25% view policy.

The middle read exposes another benefit beyond avoiding payload copies:
a view can reuse the backing's existing sparse index, while a fresh copied
substring must navigate its own characters. Results therefore combine copy
avoidance and shared Unicode navigation, rather than isolating memcpy cost.

Controls use 4,096-character windows below the retention ratio, actual `::2`
stepped slices that must materialize, and index-only access without slicing.
A retained case keeps 256 overlapping large windows after releasing the guest's
source variable. All cases check a scalar result derived independently in CPython.

## CPU results

| Input / stride       |      Main | Index PR | Optimized |
| -------------------- | --------: | -------: | --------: |
| Latin/Greek, 64      | 589.87 ms | 21.51 ms |   3.04 ms |
| CJK, 64              | 607.68 ms | 29.26 ms |   3.15 ms |
| Mixed, 1             | 348.12 ms | 25.51 ms |   3.05 ms |
| Mixed, 8             | 381.47 ms | 25.47 ms |   2.98 ms |
| Mixed, 64            | 493.02 ms | 25.54 ms |   3.12 ms |
| Mixed, 1,024         | 493.39 ms | 25.49 ms |   3.18 ms |
| Mixed, small windows | 324.68 ms |  4.22 ms |   4.27 ms |
| Mixed, `::2` windows |  38.50 ms | 38.14 ms |  38.10 ms |
| Mixed, index only    |  20.45 ms |  2.64 ms |   2.65 ms |

The controls show that copied small windows get no additional view benefit,
stepped slicing remains unchanged, and index-only gains come from the index PR.
All 24 rows are in [results.md](results.md), with samples in [cpu.csv](cpu.csv).

## Peak RSS

| 256 retained overlapping windows |      Main |  Index PR | Optimized |
| -------------------------------- | --------: | --------: | --------: |
| Latin/Greek                      | 41.31 MiB | 41.44 MiB |  9.33 MiB |
| CJK                              | 57.61 MiB | 57.72 MiB |  9.55 MiB |
| Mixed                            | 49.45 MiB | 49.58 MiB |  9.42 MiB |

Streaming one window at a time has much smaller peak differences: mixed stride
64 uses 9.70 / 9.48 / 9.27 MiB respectively. Retaining windows changes the memory
trade-off because copied payloads remain live together. Absolute peaks include
host buffers, compilation and harness/runtime storage; they are not retained
payload counts. See [rss.md](rss.md) and [rss.csv](rss.csv).

This one-backing workload does not exercise the larger per-owner lazy-cache
cost measured by the [independent-string control](../inline-value/dual-cursor-hashes/README.md).

## Measurement and reproduction

Main is freshly fetched `origin/main` at `2a326904`; it does not contain #1236.
The index-only PR control is `5c9dddf3`. The optimized build is the uncommitted
inline-view/two-cursor/adaptive-hash prototype on `f2935043`.
All are production libraries built with the optimized `profiling` profile.

Five randomized fresh processes per workload/build execute repeated calls in
one VM. Cells report median per-call macOS `wait4` user+system CPU, including
amortized startup, input import, compilation and teardown. Guest counts are five
for streaming/index cases, three for stepped slices, and two for retained cases.
Source data and its navigation caches remain available across repeated calls.
A separate five-process run measures absolute peak RSS for one original execution.

`measure.py` generates every input, program and independent CPython expectation.
All 24 original programs and all repeated-call checks pass on all three builds.
It records hashes, revisions and parameters in [provenance.json](provenance.json).
No interpreter implementation changed for this experiment.

```sh
python measure.py --prepare-only
python measure.py --main /path/to/main-harness --index /path/to/index-pr-harness \
  --optimized /path/to/optimized-harness
```

Build the harness as described in the [chunker report](../agent-chunking/README.md).
