"""Regenerate the golden dump fixtures from the builds that wrote them.

A migration cannot be tested against bytes this build produced, so each fixture
must come from a build of the commit that wrote its dump version. For every
version in the corpus this exports the recorded commit with `git archive`, drops
the generator in as an untracked test, builds and runs it, and collects the
fixtures.

`git archive` rather than a worktree writes nothing into the repository, so an
interrupted run leaves no worktree entry to prune. Build artifacts go to
`target/golden-dumps/` and are reused between runs.

Usage:
    uv run scripts/generate_golden_dumps.py [--version N] [--keep]
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
from pathlib import Path
from typing import Any, TypedDict


class Version(TypedDict):
    """One dump version in `corpus.json`, and the commit whose build wrote it."""

    version: int
    commit: str
    subject: str


ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / 'crates' / 'monty' / 'tests' / 'golden_dumps'
CORPUS = FIXTURES / 'corpus.json'
GENERATOR = ROOT / 'scripts' / 'golden_dumps' / 'generate.rs'
# Gitignored, reclaimed by `cargo clean`, and kept between runs so only the
# first regeneration pays for a cold build.
TARGETS = ROOT / 'target' / 'golden-dumps'
# Cargo discovers `tests/*.rs` itself, so the exported Cargo.toml, which must
# stay exactly as that commit had it, needs no edit.
GENERATOR_TARGET = Path('crates') / 'monty' / 'tests' / 'generate_golden_dumps.rs'


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--version', type=int, help='regenerate only this dump version')
    parser.add_argument('--keep', action='store_true', help='leave the exported sources in place for inspection')
    args = parser.parse_args()

    corpus: dict[str, Any] = json.loads(CORPUS.read_text())
    versions: list[Version] = [v for v in corpus['versions'] if args.version is None or v['version'] == args.version]
    if not versions:
        print(f'no version {args.version} in {CORPUS.relative_to(ROOT)}', file=sys.stderr)
        return 1

    for version in versions:
        generate(version, keep=args.keep)
    return 0


def generate(version: Version, *, keep: bool) -> None:
    """Build the recorded commit in a throwaway export and collect its dumps."""
    number, commit = version['version'], version['commit']
    out = FIXTURES / f'v{number}'
    print(f'==> v{number} from {commit[:12]} ({version["subject"]})', flush=True)

    verify_version(commit, number)
    source = Path(tempfile.mkdtemp(prefix=f'monty-golden-v{number}-'))
    try:
        export(commit, source)
        shutil.copy(GENERATOR, source / GENERATOR_TARGET)
        if out.exists():
            shutil.rmtree(out)
        run(
            ['cargo', 'test', '-p', 'monty', '--test', 'generate_golden_dumps'],
            cwd=source,
            env={
                **os.environ,
                'CARGO_TARGET_DIR': str(TARGETS / f'v{number}'),
                'MONTY_CORPUS': str(CORPUS),
                'MONTY_FIXTURE_OUT': str(out),
                'MONTY_FIXTURE_COMMIT': commit,
                'MONTY_FIXTURE_SUBJECT': version['subject'],
            },
        )
        written = sorted(p.name for p in out.glob('*.dump'))
        print(f'    {len(written)} fixtures: {", ".join(written)}', flush=True)
    finally:
        if keep:
            print(f'    export kept at {source}', flush=True)
        else:
            shutil.rmtree(source, ignore_errors=True)


def export(commit: str, dest: Path) -> None:
    """Extracts a commit's tree into `dest`, touching nothing in the repository.

    `git archive` stamps every file with the commit's time, which is older than
    anything in the reused target directory, so cargo would skip rebuilding a
    newly pinned commit. Every file is re-stamped with the current time.
    """
    archive = dest.parent / f'{dest.name}.tar'
    run(['git', 'archive', '--format=tar', '--output', str(archive), commit], cwd=ROOT, quiet=True)
    try:
        with tarfile.open(archive) as tar:
            tar.extractall(dest, filter='data')
    finally:
        archive.unlink(missing_ok=True)
    now = time.time()
    for path in dest.rglob('*'):
        os.utime(path, (now, now), follow_symlinks=False)


def verify_version(commit: str, expected: int) -> None:
    """Exit unless `commit`'s `DUMP_VERSION` is the version the corpus records.

    A mislabelled fixture would make every verdict built on it meaningless.
    """
    source = subprocess.run(
        ['git', 'show', f'{commit}:crates/monty/src/dump_format.rs'],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    for line in source.splitlines():
        if line.startswith('pub const DUMP_VERSION'):
            found = int(line.rstrip(';').split('=')[1])
            if found != expected:
                raise SystemExit(f'{commit[:12]} writes dump version {found}, corpus says {expected}')
            return
    raise SystemExit(f'{commit[:12]} has no DUMP_VERSION to check')


def run(
    cmd: list[str], *, cwd: Path, env: dict[str, str] | None = None, quiet: bool = False, check: bool = True
) -> None:
    """Run `cmd`, exiting on failure when `check`; `quiet` hides output unless it fails."""
    result = subprocess.run(cmd, cwd=cwd, env=env, capture_output=quiet)
    if check and result.returncode != 0:
        if quiet and result.stderr:
            sys.stderr.write(result.stderr.decode())
        raise SystemExit(f'{" ".join(cmd)} failed with {result.returncode}')


if __name__ == '__main__':
    raise SystemExit(main())
