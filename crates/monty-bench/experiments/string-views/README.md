# String views on Unicode access caches

The latest inline-view implementation is measured by the [agent document chunker](agent-chunking/README.md),
[overlapping UTF-8 strides](utf8-strides/README.md), and [optional view-cache comparison](inline-value/lazy-view-cache/README.md).

The report below describes the historical `Arc<str>` implementation at `f7be4e56`.
The `Arc<String>` storage comparison is reported in [owned-string/](owned-string/).
The earlier splitlines exception and four-way comparison are in [splitlines/](splitlines/).

The candidate is based on PR #1236 at `5c9dddf39c92981b8ef023be059e8c3aeabf7e91`.
Heap strings use `Arc<str>` with a flattened byte range.
The production policy requires at least 16 UTF-8 bytes and one quarter of the original backing allocation.

## Results

Two processes per build, nine rounds each: 1,386 timing samples and 66 candidate metric records.
The table uses the median across both processes.
The candidate's explicit 16-byte / 25% policy matches the production default.

| Workload                | #1236 baseline | Views 16 B / 25% | Speedup |
| ----------------------- | -------------: | ---------------: | ------: |
| docx_selected           |       1.388 ms |         1.427 ms |   0.97x |
| docx_text               |       1.380 ms |         1.443 ms |   0.96x |
| json_preview_retained   |       0.476 ms |         0.481 ms |   0.99x |
| json_request            |       2.301 ms |         2.090 ms |   1.10x |
| json_uri_normalize      |       3.264 ms |         2.962 ms |   1.10x |
| ndjson_ascii_splitlines |       3.944 ms |         3.763 ms |   1.05x |
| ndjson_ascii_tail       |      36.476 ms |         4.801 ms |   7.60x |
| ndjson_utf8_splitlines  |       3.141 ms |         2.972 ms |   1.06x |
| ndjson_utf8_tail        |      61.557 ms |         4.153 ms |  14.82x |
| template_fragments      |       1.278 ms |         1.314 ms |   0.97x |
| template_styled         |       1.294 ms |         1.341 ms |   0.96x |

The UTF-8 prefix-consumption case improves 14.8x over #1236 alone, and the ASCII version improves 7.6x.
`splitlines()` remains faster than repeated prefix removal for both encodings.
The earlier view implementation without #1236 took roughly 764 ms on the UTF-8 prefix case; that historical
comparison came from a different run and is not included in the table.

Substring character counts must survive slicing: #1236 computes normalized character bounds before translating
them to bytes, so the new string can seed its length cache without scanning the substring again.
Keeping fresh navigation state but discarding that count took about 45 ms in the same UTF-8 scenario.

Large-request processing improves about 10%, including the change to construct Arc strings directly from borrowed
JSON decoder text instead of first allocating temporary strings.
The template and XML kernels regress about 3–5% in these runs; candidate timings retain dormant test-only
counter hooks, and these small differences need confirmation in production builds.
These kernels do not establish an end-to-end Jinja2 or DOCX package speedup.

The retained preview owns 270 UTF-8 bytes under the heuristic, versus retaining 760,000 backing bytes with always-view.
The production policy has no minimum beyond the existing 15-byte inline limit, while retaining the 25% ratio guard.
Higher minimums remain in the benchmark matrix for comparison.

[samples.csv](samples.csv) contains timing rows; [metrics.csv](metrics.csv) contains backing-payload measurements.
[summary.csv](summary.csv) summarizes every policy, and [provenance.json](provenance.json) records baseline revision,
input hashes and benchmark executable hashes.

## Method

The ignored `benchmark_string_realistic_scenarios` library test generates deterministic host-input fixtures.
Each policy uses the same Unicode cursor and sparse-index implementation from #1236.
A substring seeds its known character count without sharing navigation state.
The copy policy disables view creation while retaining the candidate representation and JSON decoder changes.
It is a policy control, not the unchanged #1236 baseline.

The optimized `profiling` build excludes compilation from timings, warms each case twice, calibrates batches to
approximately 5 ms (at most 100 executions), and shuffles case/policy order for nine rounds.
Host input cloning/import and output checking are included; source compilation is excluded except in the retained-session case.
Local timing differences of a few percent need confirmation across repeated runs.

Backing metrics count string-storage allocations and payload bytes, not all allocator calls or RSS.
`slice_materialized` counts explicit substring copies; `owned_finalize` counts an additional copy from an owned
construction buffer into its final Arc allocation.
`peak_with_owned` includes that buffer overlap but excludes other temporary buffers, headers, cache storage and containers.
Retained cases sample live string backings after releasing their original input while keeping the REPL alive.

## Workloads

- Templates substitute variables and escape HTML in generated documents, with and without a large static style block.
    This is a Jinja-style kernel, not a full Jinja2 integration.
- DOCX cases scan valid WordprocessingML after host-side decompression, unescape text runs, and optionally retain selected runs.
    They do not run python-docx or ZIP decompression in Monty.
- JSON requests parse roughly 1 MB of events, aggregate values and encode an error response.
    The URI variant additionally removes a short prefix from medium-sized strings.
- NDJSON cases compare consuming a buffer through successive slices with a `splitlines()` control, using ASCII-escaped
    and raw multibyte UTF-8 versions of the same data.
- The preview case retains 256 Unicode characters from a large decoded JSON string after releasing the request.

CPython checks the scalar outputs, complete template output and extracted XML text independently.
These workloads keep the Unicode indexing work unchanged; substrings retain their known character count and receive fresh navigation cursors.

## Reproduction

```sh
cargo test -p monty --profile profiling --lib benchmark_string_realistic_scenarios -- \
  --ignored --nocapture --test-threads=1
```

Set `MONTY_SCENARIO_EXPORT_DIR` to export the generated Python sources, input data and expected results for independent checks.
The separate `benchmark_string_view_runtime_matrix` ignored test exercises minimum sizes 32/64/128/256 and retention
fractions 12.5%/25%/50%, plus always-copy and always-view controls.

For the unchanged baseline, create a separate checkout of the revision above, then run
`python3 install-baseline-harness.py /path/to/baseline` from this directory.
Run the same benchmark command in that checkout; its `base` rows use the unchanged production implementation.
Use separate Cargo target directories, or preserve the compiled benchmark executables before switching worktrees:
Cargo outputs with the same name can otherwise overwrite one another in a shared target directory.
Baseline metric rows have no probes and must not be interpreted as zero allocations.
