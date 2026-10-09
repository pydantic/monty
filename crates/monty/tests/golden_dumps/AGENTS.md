# The dump corpus

Real dumps from the builds that wrote them, and the verdict each one must get from this build.

The corpus is a fixed measurement for comparing approaches to reading old dumps.
A branch implementing an approach rebases onto it, changes no code in `tests/common/dump_corpus.rs` or
`tests/dump_corpus.rs`, and edits only the `verdicts` rows in `corpus.json`.
The diff between two branches' copies of that file is the comparison.

## What is here

| File                 | What it is                                                                          |
| -------------------- | ----------------------------------------------------------------------------------- |
| `corpus.json`        | Hand-maintained: the cases, each version's commit, and each case's `verdicts` row.  |
| `cases/<name>.py`    | Hand-maintained: the source each case feeds before it is dumped.                    |
| `v<N>/*.dump`        | Generated: real bytes from a checkout of that version's commit.                     |
| `v<N>/manifest.json` | Generated: each global's `repr` and the feed result, as the writing build saw them. |

## Verdicts

`verify_restore` returns one of four verdicts:

- **`loads`**: the restored session agrees with the one that was dumped.
- **`refused`**: `Dump::load` returned an error.
    This is a legitimate outcome for an approach, not a failure.
- **`corrupt`**: loaded into a session that disagrees with the dumped one.
- **`panicked`**: the load panicked.
    A dump is untrusted input, so a panic is a failure to reject it, not a rejection.

The suite exists to rule out the last two, so `corpus.json` cannot express either: `corpus()` rejects a `verdicts` row
that names one.

Decoding successfully is not enough.
A payload can decode and still restore globals under the wrong names, so each check goes through the session:
every recorded global is read back with `repr`, the case's `feed` snippet is run, and the session is dumped and
reloaded at the current version.

## Adding a case

Write `cases/<name>.py`, add a matching entry with its `verdicts` row to `corpus.json`, and run
`make generate-golden-dumps`.
`every_case_has_a_python_file` fails on a file with no entry and on an entry with no file.
`the_corpus_describes_itself` fails on a case with no verdict for some version.

`MontyRepl` keeps the source it was fed, so reformatting a case source changes the dumped bytes.
Run `make generate-golden-dumps` after `ruff format` touches one.

Every case is dumped at every version, so its source must run on the oldest.

## Regenerating

`make generate-golden-dumps` exports each recorded commit with `git archive`, copies
`scripts/golden_dumps/generate.rs` in as an untracked test, builds and runs it, and copies the fixtures back.
Only a build of the old commit writes its bytes, so a version's fixtures must be made while its commit still builds.

`git archive` rather than a worktree because it writes nothing into the repository: an interrupted run leaves only a
temporary directory, with no worktree entry to prune.
Build artifacts go to `target/golden-dumps/v<N>`, which `cargo clean` removes and which is reused between runs.
The export is re-stamped with the current time, because `git archive` dates files to the commit and cargo would
otherwise treat a newly pinned commit as already built; so the monty crates rebuild every run and only dependencies
are cached.
A cold regeneration takes about half a minute per version and a warm one about ten seconds, at roughly 1 GB per
version; delete `target/golden-dumps` to reclaim it.

**Regenerating does not reproduce the same bytes.**
`MontyRepl` holds `ahash` maps, which are randomly seeded and serialize in iteration order, so the same session
dumped twice on the same build differs.
The harness compares the recorded `repr`s and feed result, which are stable.
Do not add a CI check that diffs the fixtures; it would fail every run.

## Adding a version

Record the last commit that wrote it, which is the one before the next bump.
Check the bump is real: version 9 covered two incompatible shapes, because `cwd` and `random` were added to
`MontyRepl` after it without a bump of their own, and version 11 did too, because #880 split
`ResourceLimits.max_duration`.
To check, dump the cases at the version's first commit and load them at the one you record.
`scripts/generate_golden_dumps.py` checks that the commit writes the version the corpus claims, but it cannot see a
shape that changed without a bump.
