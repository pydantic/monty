"""Fetch pinned upstream files and apply reasoned patches to checked-in vendor trees."""

import difflib
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.request
from io import StringIO
from pathlib import Path
from urllib.error import HTTPError

REPO = Path(__file__).parent.parent
RAW_URL = 'https://raw.githubusercontent.com/{repository}/{commit}/{path}'


def fetch(repository: str, commit: str, path: str) -> str:
    """Fetch a pinned file, retrying transient HTTP failures."""
    url = RAW_URL.format(repository=repository, commit=commit, path=path)
    delay = 1
    while True:
        try:
            with urllib.request.urlopen(url, timeout=30) as response:
                return response.read().decode('utf-8')
        except HTTPError as error:
            if error.code not in (429, 500, 502, 503, 504) or delay > 4:
                raise
            error.close()
            print(f'{repository} download for {path} returned HTTP {error.code}; retrying in {delay}s', file=sys.stderr)
            time.sleep(delay)
            delay *= 2


def fetch_sources(repository: str, commit: str, directory: str, files: list[str]) -> dict[str, str]:
    """Fetch files keyed by paths relative to the upstream directory."""
    return {file: fetch(repository, commit, f'{directory}/{file}') for file in files}


def sync_tree(expected: dict[Path, str], roots: tuple[Path, ...], *, check: bool, hint: str) -> None:
    """Check or rewrite the expected files, treating extra files under `roots` as stale."""
    stale = sorted(path for root in roots for path in root.rglob('*') if path.is_file() and path not in expected)
    if check:
        changed = [path for path, text in expected.items() if read(path) != text] + stale
        if changed:
            listed = '\n'.join(f'    {path.relative_to(REPO)}' for path in changed)
            sys.exit(f'{hint}:\n{listed}')
    else:
        for path in stale:
            path.unlink()
        for path, text in expected.items():
            write(path, text)


def read(path: Path) -> str | None:
    """Read exact text without normalizing line endings; return `None` for a missing file."""
    return path.read_bytes().decode('utf-8') if path.is_file() else None


def write(path: Path, text: str) -> None:
    """Write UTF-8 without platform line-ending conversion."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding='utf-8', newline='\n')


class PatchError(Exception):
    """A patch is malformed or does not apply."""


class PatchDoesNotApply(PatchError):
    """A patch's context is missing, e.g. because an earlier patch it builds on was dropped."""


def apply_patches(sources: dict[str, str], patches: list[Path], *, additions: tuple[str, ...] = ()) -> dict[str, str]:
    """Apply ordered patches without fuzz; only declared additions may create new files."""
    allowed = sources.keys() | set(additions)
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for file, text in sources.items():
            write(root / file, text)
        for patch in patches:
            check_patch(patch, allowed)
            git_apply(patch, root)
        result = {
            path.relative_to(root).as_posix(): path.read_bytes().decode('utf-8')
            for path in sorted(root.rglob('*'))
            if path.is_file()
        }
        if unexpected := result.keys() - allowed:
            raise PatchError(f'patches created unlisted files: {", ".join(sorted(unexpected))}')
        if missing := sources.keys() - result.keys():
            raise PatchError(f'patches removed upstream files: {", ".join(sorted(missing))}')
        return result


def check_patch(patch: Path, allowed: set[str]) -> None:
    """Require a reason and reject edits to unlisted files."""
    text = patch.read_text(encoding='utf-8')
    if text.lstrip().startswith(('--- ', 'diff ')) or not text.strip():
        raise PatchError(f'{patch.relative_to(REPO)} must start with the reason it is needed')
    for line in text.split('\n'):
        if line.startswith(('--- a/', '+++ b/')) and line[6:] not in allowed:
            raise PatchError(f'{patch.relative_to(REPO)} edits {line[6:]}, which vendor.toml does not list')


def git_apply(patch: Path, root: Path) -> None:
    """Apply a patch without using an enclosing checkout as the target."""
    env = os.environ | {'GIT_CEILING_DIRECTORIES': str(root.parent)}
    result = subprocess.run(
        ['git', 'apply', '--whitespace=error-all', patch],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode:
        raise PatchDoesNotApply(f'{patch.relative_to(REPO)} does not apply:\n{result.stderr}')


def make_patch(
    name: str,
    reason: str,
    upstream: dict[str, str],
    source_dir: Path,
    patch_dir: Path,
    *,
    additions: tuple[str, ...] = (),
) -> None:
    """Save edits against the preceding patches; only the last patch can be replaced."""
    if not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.-]*', name):
        sys.exit('--make-patch needs a filename without directory components')
    target = patch_dir / f'{name}.patch'
    patches = sorted(patch_dir.glob('*.patch'))
    if later := [patch.name for patch in patches if patch.name > target.name]:
        sys.exit(f'{target.name} would sort before {", ".join(later)}; pick a name that sorts last')
    base = apply_patches(upstream, [patch for patch in patches if patch != target], additions=additions)
    hunks: list[str] = []
    for file in sorted(upstream.keys() | set(additions)):
        old = base.get(file)
        new = read(source_dir / file)
        if old == new:
            continue
        if new is None:
            sys.exit(f'{source_dir.relative_to(REPO) / file} is missing')
        if not new.endswith('\n') or (old is not None and not old.endswith('\n')):
            # difflib cannot emit Git's marker for a missing final newline.
            sys.exit(f'{source_dir.relative_to(REPO) / file} must end with a newline in both trees')
        hunks.extend(
            difflib.unified_diff(
                StringIO(old or '').readlines(),
                StringIO(new).readlines(),
                '/dev/null' if old is None else f'a/{file}',
                f'b/{file}',
            )
        )
    if not hunks:
        sys.exit(f'{source_dir.relative_to(REPO)} has no edits beyond the existing patches')
    write(target, f'{reason.strip()}\n\n{"".join(hunks)}')
    print(f'wrote {target.relative_to(REPO)}')


def patched_files(patch: Path) -> set[str]:
    """Return the paths a patch creates or edits."""
    return {line[6:] for line in patch.read_text(encoding='utf-8').split('\n') if line.startswith('+++ b/')}
