# Agent document chunking with overlapping context

A paragraph-aware retrieval chunker is a favorable real algorithm for inline
string views. It consumes a multilingual Markdown document, chooses paragraph
or word boundaries, and retains 256 characters of context between chunks.
This is custom preprocessing an agent could write before sending chunks to an
embedding service; it does not reimplement a standard-library parser.

For the 2 MiB document, the remainder-consuming version takes 1.73 ms with the
optimized Monty build, versus 58.7 ms with the UTF-8 index PR alone, 1,048 ms on
current main, and 94.0 ms on CPython 3.14.2. The offset-based control is faster
on CPython: 0.65 ms versus optimized Monty's 1.50 ms.

## Algorithm and input

[remainder.py](remainder.py) is the straightforward implementation:

```python
end = text.rfind('\n\n', max_chars // 2, max_chars)
# Fall back to a word boundary or the size limit.
chunks.append((offset, text[:end]))
advance = end - overlap
text = text[advance:]
offset += advance
```

Returned chunks have at most 2,000 Python characters and retain exact source
character offsets. The generated document contains numbered headings, English
and German prose, Japanese text and emoji. It is synthetic Markdown with a
real chunking policy, rather than an end-to-end retrieval/embedding benchmark.
The three inputs are approximately 256 KiB, 1 MiB and 2 MiB of UTF-8; the largest
contains 1,552,325 characters and produces 932 overlapping chunks.

The large shrinking remainder meets the ordinary 16-byte / 25% view policy
until a smaller backing is needed. Returned chunks are generally below 25%
and still copy. This targets repeated tail copies and shared Unicode navigation;
it does not require lifting the retention ratio for small chunks.

[offsets.py](offsets.py) implements the same policy while retaining the original
string and advancing an integer offset. Both versions produce identical full
chunk texts and offsets. This control shows how much the consuming style costs
when substring creation copies, and avoids implying that views beat CPython's
more efficient offset-based implementation.

## Execution time

Five randomized fresh processes per case/build report median per-call
user+system CPU from macOS `wait4`. Each process repeats the same guest workload
in one VM/interpreter. Batches are calibrated toward 500 ms, capped at 2,048
calls; slow cases execute once per process. Startup, fixture loading, compilation,
input conversion and teardown are amortized but included. Source input and its
Unicode caches persist across repeated calls. A scalar digest checks each result;
this digest work is included, and full output validation is separate.

| Input / implementation |        Main | Index PR | Optimized | CPython 3.14.2 |
| ---------------------- | ----------: | -------: | --------: | -------------: |
| 256 KiB, remainder     |    17.22 ms |  1.15 ms |   0.21 ms |        1.09 ms |
| 1 MiB, remainder       |   267.92 ms | 15.15 ms |   0.84 ms |       20.86 ms |
| 2 MiB, remainder       | 1,048.27 ms | 58.68 ms |   1.73 ms |       94.03 ms |
| 2 MiB, offsets         | 1,900.27 ms |  1.48 ms |   1.50 ms |        0.65 ms |

For the large consuming case, views add about 34x over the index PR, and the
complete stack is about 54x faster than CPython executing that same version.
Doubling from 1 MiB to 2 MiB takes roughly four times as long with copied tails
and roughly twice as long with views. The offset-based implementation already
avoids repeated tail copies and gets no comparable additional view benefit.

These are kernel results. Pool/RPC overhead, embedding requests and document
loading from a host filesystem are outside the guest workload. They are not
claims about an entire agent application. Small CPython rows retain more
startup overhead because their calibrated batches reach the call-count cap.
All six timings are in [results.md](results.md), with per-process counts and
samples in [cpu.csv](cpu.csv).

## Peak RSS

A separate run executes each original workload once per fresh process, five
times per build. Numbers are absolute process peaks in MiB, including the
runtime, harness, host input buffers, compilation and result storage.
CPython's runtime/harness footprint differs from the Rust harness; these are
not measurements of substring payload alone or post-collection retained memory.

| 2 MiB input |      Main |  Index PR | Optimized |   CPython |
| ----------- | --------: | --------: | --------: | --------: |
| Remainder   | 23.77 MiB | 21.59 MiB | 16.22 MiB | 60.89 MiB |
| Offsets     | 15.55 MiB | 15.77 MiB | 15.77 MiB | 34.12 MiB |

See [rss.md](rss.md) and [rss.csv](rss.csv). The CSV also records CPU time for
the single-execution RSS runs, separately from repeated-call throughput.

## Validation and reproduction

Current main was refreshed from origin before building: `2a326904`.
The middle control is #1236 at `5c9dddf3`. The optimized executable is the
uncommitted inline-view, two-cursor and adaptive-hash prototype on `f2935043`.
All Monty production libraries use the same optimized `profiling` profile,
without the library's test-only string counters. CPython is the local 3.14.2
interpreter. Machine, executable/input/source hashes, calibrated batch counts
and revisions are recorded in [provenance.json](provenance.json).

`prepare.py` independently checks coverage, size limits, overlap and every
source character offset for both CPython implementations. It also checks empty,
unbroken-word, delimiter-only and multibyte inputs. Every measured process
checks the expected scalar output. The additional Rust integration test in
`validation-harness.rs`, run with `memory-model-checks`, compares all six complete
Monty chunk lists against CPython's exported text/offset lists.

```sh
python prepare.py
python measure.py --main /path/to/main-harness --index /path/to/index-pr-harness \
  --optimized /path/to/optimized-harness --python /path/to/python3.14
```

Build the Monty executables by installing `../splitlines/rss-harness.rs` as
`crates/monty/tests/peak_rss.rs` in each isolated worktree, then running
`cargo test -p monty --profile profiling --test peak_rss --no-run`.
Freeze each executable at a distinct path before changing worktrees when
sharing a Cargo target directory. The validation harness is temporarily installed
as `crates/monty/tests/agent_chunking_validation.rs`; point
`MONTY_CHUNKING_FIXTURES` to the generated `fixtures/` directory.
