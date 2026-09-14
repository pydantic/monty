# nested_rlm

A 2 MB application log bound as `context`, answered by delegating chunks to a child model.
Split the log into chunks, ask `rlm_query(prompt, chunk)` to count ERROR lines by code and host in each, gather the
replies, merge the counts in code, and return the most common code, its count, and the host that produced most of it.

`rlm_query` is a task tool; under `--dry-run` it is a deterministic count of the chunk's ERROR lines returned as JSON.
It is a sub-model call over the chunk, not a child REPL with its own iteration: a true recursive child needs a model
to write its code, which the dry run has no way to supply.

Scored with `ApproxExpected`, `result_size` (200 bytes) and `within_call_budget` of two waves, so a chunk-by-chunk
sequential loop fails.
