# Snapshot replay

Record synchronous host calls, restore a saved suspension in a fresh Monty worker, and compare a changed response.
The included program reads public PyPI metadata for three packages and returns their Monty dependencies.
Replay uses the recorded responses; it never calls PyPI.
No model or API key is required.

## Run

From the repository root, build the current worker and Python client with `make dev-py`.
Pass the trusted worker explicitly; use `target/debug/monty.exe` instead on Windows.

Only load recordings you created and kept private.
A recording contains source, arguments, responses, printed output and opaque worker snapshots.
The checksums detect corruption, not malicious replacement.
Files remain on disk until you delete them; this example provides no automatic deletion, redaction or encryption.

```bash
mkdir -p examples/snapshot_replay/recordings
uv run python -m examples.snapshot_replay.main --binary target/debug/monty capture examples/snapshot_replay/recordings/run.jsonl
uv run python -m examples.snapshot_replay.main --binary target/debug/monty replay examples/snapshot_replay/recordings/run.jsonl
uv run python -m examples.snapshot_replay.main --binary target/debug/monty branch examples/snapshot_replay/recordings/run.jsonl --at 1 --response examples/snapshot_replay/response.json --output examples/snapshot_replay/recordings/branch.json
uv run python -m examples.snapshot_replay.main --binary target/debug/monty report examples/snapshot_replay/recordings/run.jsonl --branch examples/snapshot_replay/recordings/branch.json --output examples/snapshot_replay/recordings/report.html
```

In PowerShell, create the directory with `New-Item -ItemType Directory examples/snapshot_replay/recordings`.
Open `report.html` locally to inspect the calls and result differences.
It contains no scripts or remote assets.
The comparison file includes the replacement response or edited source as well as the original recording's checksum.
Outputs use exclusive creation: choose new filenames for another capture or comparison.
The response in `response.json` is deliberately hypothetical, not a claim about the package on PyPI.

To compare edited sandbox code, pass `--code path/to/edited.py` to `replay`.
That starts from the edited source rather than restoring bytecode from the original snapshot.
The name, host-visible JSON arguments, mapping order and order of every host call must still match the recording.
A mismatch reports `DIVERGED`; it does not fetch another response.
Monty converts some guest values, such as functions, to strings before the host receives them.
Replay cannot distinguish those values from literal strings with the same text; see
[host-function argument conversion](../../docs/host-functions.md#arguments-and-return-values).

## Boundaries

`rewind.py` owns the journal and replay logic.
`pypi_tools.py` owns the live HTTP callback and permits only the three named packages at a fixed PyPI endpoint.
The callback runs on the host with host authority, as described in [Monty's security model](../../docs/security.md).
Changing it requires validating the sandbox's arguments and bounding host work independently of Monty's limits.

Each call and snapshot is flushed before the host callback runs.
Loading rejects empty or truncated recordings and reports a missing response or completion record.
A missing response does not establish whether the callback ran.
Do not retry a side-effecting callback merely because its response is missing.

This example accepts finite JSON values at the host boundary, up to 16 direct synchronous host calls, 32 KiB of source,
256 KiB per value or captured output, 512 KiB per snapshot and 8 MiB per recording.
It rejects host objects, OS calls, name-lookup suspensions and futures.
It does not inspect frames or locals, schedule async completions or persist host state.

Replay requires the same Python client version and exact worker binary as capture.
It is not a cross-version snapshot format.
Restoring a later suspension preserves the recorded output prefix and remaining call count.
The snapshot carries its resource limits and accumulated execution time; restoring does not reset the time budget.
Memory limits apply to live allocations in the restored worker.
Two different error observations are reported as different, even if both mention a resource limit.

## Tests

The regression suite uses real workers with deterministic host responses and does not call PyPI.

```bash
uv run --package pydantic-monty-client --only-dev python -m pytest crates/monty-python/tests/test_snapshot_replay_example.py
uv run ruff check examples/snapshot_replay/main.py examples/snapshot_replay/rewind.py examples/snapshot_replay/pypi_tools.py crates/monty-python/tests/test_snapshot_replay_example.py
uv run ruff format --check examples/snapshot_replay/main.py examples/snapshot_replay/rewind.py examples/snapshot_replay/pypi_tools.py crates/monty-python/tests/test_snapshot_replay_example.py
```
