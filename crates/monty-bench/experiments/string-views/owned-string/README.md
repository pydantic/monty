# Owned/view storage using Arc<String>

This storage experiment is based on PR #1246 at `f7be4e56439039296812a6160638be9fd0d5a58e`, itself based on #1236.
Whole strings store `Owned(Arc<String>)`; slices store the same backing plus a byte offset and length.
The storage enum measures 24 bytes on this arm64 build, versus 32 bytes for the published Arc<str> representation.
The enum still reserves room for its largest variant; an individual Owned value does not make the heap entry smaller than that.

Owned buffers call `shrink_to_fit()` and move into the Arc without an unconditional finalization copy.
Shrinking can itself move/copy the buffer, so this does not guarantee zero byte copies for overallocated inputs.
The retention test uses actual `String::capacity()`, since shrinking need not remove all spare capacity.
Both variants retain the 16-byte / 25% policy, flattened views, immutable bytes and text-only snapshots.
Borrowed text now needs a byte-buffer allocation plus a separate Arc header.

## Timing

The two representations were rerun in alternating order across three processes each, with nine rounds per process.
The following medians use the 16-byte / 25% policy: 27 timing samples per cell.
All six policies were measured, for a total of 3,564 timing samples.
As in the original report, dormant test-only counter hooks remain in these timing executables.
Small differences of a few percent remain indicative local measurements.

| Workload                | Arc<str> | Arc<String> + shrink | Time change |
| ----------------------- | -------: | -------------------: | ----------: |
| docx_selected           | 1.409 ms |             1.445 ms |       +2.5% |
| docx_text               | 1.423 ms |             1.434 ms |       +0.7% |
| json_preview_retained   | 0.469 ms |             0.468 ms |       -0.3% |
| json_request            | 2.062 ms |             2.093 ms |       +1.5% |
| json_uri_normalize      | 2.917 ms |             3.020 ms |       +3.6% |
| ndjson_ascii_splitlines | 3.702 ms |             3.656 ms |       -1.2% |
| ndjson_ascii_tail       | 4.706 ms |             4.715 ms |       +0.2% |
| ndjson_utf8_splitlines  | 2.924 ms |             2.895 ms |       -1.0% |
| ndjson_utf8_tail        | 4.014 ms |             4.024 ms |       +0.2% |
| template_fragments      | 1.298 ms |             1.308 ms |       +0.8% |
| template_styled         | 1.320 ms |             1.344 ms |       +1.8% |

## Peak RSS

Five fresh processes per workload/build, randomized, using production-library profiling builds without counter hooks.
The table gives median absolute process peak RSS from macOS wait4(2), in MiB (2^20 bytes).
Each process loads and executes one fixture once; peaks include loading, guest compilation and result checking.
The empty-harness footprint is not subtracted.
The baseline is unchanged #1236; the Arc<str> column is published PR #1246.

| Workload                |     #1236 |  Arc<str> | Arc<String> + shrink | Change vs Arc<str> |
| ----------------------- | --------: | --------: | -------------------: | -----------------: |
| empty                   |  6.27 MiB |  6.28 MiB |             6.22 MiB |              -1.0% |
| template_styled         |  9.34 MiB |  9.45 MiB |             9.39 MiB |              -0.7% |
| template_fragments      |  9.14 MiB |  9.25 MiB |             9.23 MiB |              -0.2% |
| docx_text               |  9.27 MiB |  9.47 MiB |             9.34 MiB |              -1.3% |
| docx_selected           |  9.33 MiB |  9.55 MiB |             9.41 MiB |              -1.5% |
| json_request            | 15.33 MiB | 15.38 MiB |            15.53 MiB |              +1.0% |
| json_uri_normalize      | 16.28 MiB | 16.42 MiB |            16.58 MiB |              +1.0% |
| ndjson_ascii_tail       | 13.77 MiB | 13.41 MiB |            12.44 MiB |              -7.2% |
| ndjson_ascii_splitlines | 12.80 MiB | 13.92 MiB |            13.06 MiB |              -6.2% |
| ndjson_utf8_tail        | 15.12 MiB | 12.59 MiB |            12.30 MiB |              -2.4% |
| ndjson_utf8_splitlines  | 12.36 MiB | 13.53 MiB |            12.59 MiB |              -6.9% |
| json_preview_retained   | 11.61 MiB | 11.66 MiB |            11.66 MiB |              +0.0% |

The Arc<String> trial recovers most of the splitlines peak-RSS regression: UTF-8 is 12.59 MiB versus the baseline's
12.36 MiB, compared with 13.53 MiB for Arc<str>.
JSON peaks rise about 1% over Arc<str>, with 1.5–3.6% slower execution in the request/URI cases.
This is a memory/throughput trade-off rather than an improvement in every workload.

## Accounting and validation

In this trial, backing_allocations counts two final backing components per materialized string: byte buffer and shared header.
Adopted buffers may have been allocated before the constructor; the counter is not a count of new constructor allocation calls.
Payload metrics use retained capacity and exclude headers, allocator rounding, internal shrink reallocations and their overlap.
owned_finalize is zero because the explicit String-to-Arc byte copy was removed; allocator-internal copying is not observed.
RSS remains the independent process-memory measurement.

The buffer-adoption test checks that an exact-capacity String retains its data pointer.
A separate test models spare capacity surviving shrink and verifies that it participates in the retention ratio.
All 1,046 interpreter/doc tests with memory-model checks, 105 subprocess tests and 20 Monty string fixtures pass.
The memory-limit cases show 6 MiB repetition/padding buffers fitting within an 8 MiB limit without a finalization copy.
General format padding can still create other intermediate strings and is not covered by that claim.
Rust formatting, workspace clippy in both configurations and import checks pass.

The original timing test and reproduction instructions are in the parent report.
[timing-summary.csv](timing-summary.csv) contains every policy; [timing-samples.csv](timing-samples.csv) contains raw rows.
[rss-samples.csv](rss-samples.csv) contains all 180 process peaks across the three builds.
[provenance.json](provenance.json) and [rss-provenance.json](rss-provenance.json) record source/executable hashes and method.
