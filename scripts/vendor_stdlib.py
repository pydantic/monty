"""
Vendor the CPython stdlib modules listed in `crates/monty/stdlib/vendor.toml`.

Downloads each listed file from `Lib/` at the pinned CPython tag, applies `crates/monty/stdlib/patches/*.patch` in
file-name order, and writes the result to `crates/monty/stdlib/Lib/`, along with CPython's `LICENSE`.
It also generates `crates/monty/src/modules/vendored/sources.rs`, the table the interpreter imports them from.
Files under `Lib/` that are no longer listed are deleted.

- `make vendor-stdlib`: write everything.
- `make check-stdlib` (`--check`): fail instead of writing if anything differs.
- `--make-patch NAME --reason TEXT`: save the edits made to `Lib/` since the last vendoring as `patches/NAME.patch`,
    replacing a patch of that name.
"""

import argparse
import difflib
import os
import subprocess
import sys
import tempfile
import tomllib
import urllib.request
from pathlib import Path

REPO = Path(__file__).parent.parent
STDLIB = REPO / 'crates' / 'monty' / 'stdlib'
MANIFEST = STDLIB / 'vendor.toml'
LIB = STDLIB / 'Lib'
PATCHES = STDLIB / 'patches'
LICENSE = STDLIB / 'LICENSE'
TABLE = REPO / 'crates' / 'monty' / 'src' / 'modules' / 'vendored' / 'sources.rs'
RAW_URL = 'https://raw.githubusercontent.com/python/cpython/{tag}/{path}'


def main() -> None:
    parser = argparse.ArgumentParser(description='Vendor CPython stdlib modules into crates/monty/stdlib.')
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--check', action='store_true', help='fail if the vendored files are out of date')
    mode.add_argument('--make-patch', metavar='NAME', help='save edits to Lib/ as patches/NAME.patch')
    parser.add_argument('--reason', help='why the patch is needed; required with --make-patch')
    args = parser.parse_args()

    manifest = tomllib.loads(MANIFEST.read_text(encoding='utf-8'))
    tag: str = manifest['tag']
    files: list[str] = sorted(manifest['files'])
    upstream = {file: fetch(tag, f'Lib/{file}') for file in files}

    if args.make_patch:
        if not (args.reason or '').strip():
            sys.exit('--make-patch needs a --reason explaining why the CPython source must change')
        make_patch(args.make_patch, args.reason, upstream)
        return

    expected = {LICENSE: fetch(tag, 'LICENSE'), TABLE: render_table(tag, files)}
    expected |= {LIB / file: text for file, text in apply_patches(upstream, sorted(PATCHES.glob('*.patch'))).items()}
    stale = [path for path in LIB.rglob('*') if path.is_file() and path not in expected]

    if args.check:
        changed = [path for path, text in expected.items() if read(path) != text] + stale
        if changed:
            listed = '\n'.join(f'    {path.relative_to(REPO)}' for path in changed)
            sys.exit(f'vendored stdlib is out of date, run `make vendor-stdlib`:\n{listed}')
        print(f'vendored stdlib matches CPython {tag}')
    else:
        for path in stale:
            path.unlink()
        for path, text in expected.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding='utf-8', newline='\n')
        print(f'vendored {len(files)} files from CPython {tag}')


def fetch(tag: str, path: str) -> str:
    """One file from the CPython repository at `tag`."""
    with urllib.request.urlopen(RAW_URL.format(tag=tag, path=path), timeout=30) as response:
        return response.read().decode('utf-8')


def read(path: Path) -> str | None:
    """A file's exact text, or `None` if it does not exist; unlike `read_text()`, it keeps CRLF line endings."""
    return path.read_bytes().decode('utf-8') if path.is_file() else None


def apply_patches(sources: dict[str, str], patches: list[Path]) -> dict[str, str]:
    """`sources` (keyed by `Lib/`-relative path) with `patches` applied in order by `git apply`, which never fuzzes.

    Fails if a patch has no reason before its first hunk, touches an unlisted file, or does not apply cleanly,
    as happens when the pinned tag changes the lines it edits.
    """
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        for file, text in sources.items():
            (root / file).parent.mkdir(parents=True, exist_ok=True)
            (root / file).write_text(text, encoding='utf-8', newline='\n')
        for patch in patches:
            check_patch(patch, sources)
            git_apply(patch, root)
        return {file: (root / file).read_text(encoding='utf-8') for file in sources}


def check_patch(patch: Path, sources: dict[str, str]) -> None:
    """Fails unless `patch` explains itself and only edits vendored files."""
    text = patch.read_text(encoding='utf-8')
    if text.startswith(('--- ', 'diff ')) or not text.strip():
        sys.exit(f'{patch.relative_to(REPO)} must start with the reason it is needed')
    for line in text.splitlines():
        if line.startswith(('--- a/', '+++ b/')) and line[6:] not in sources:
            sys.exit(f'{patch.relative_to(REPO)} edits {line[6:]}, which vendor.toml does not list')


def git_apply(patch: Path, root: Path) -> None:
    """Applies `patch` to the files under `root`, which is outside any repository."""
    # stop git from treating an enclosing checkout as the target, should the temp dir be inside one
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
        sys.exit(f'{patch.relative_to(REPO)} does not apply:\n{result.stderr}')


def make_patch(name: str, reason: str, upstream: dict[str, str]) -> None:
    """Saves the difference between the patched upstream files and `Lib/` as `patches/<name>.patch`.

    The earlier patches are applied first, so the new one holds only the edits they do not already make.
    Only the last patch can be replaced: later ones were made against its result.
    """
    target = PATCHES / f'{name}.patch'
    patches = sorted(PATCHES.glob('*.patch'))
    if later := [patch.name for patch in patches if patch.name > target.name]:
        sys.exit(f'{target.name} would sort before {", ".join(later)}; pick a name that sorts last')
    base = apply_patches(upstream, [patch for patch in patches if patch != target])
    hunks: list[str] = []
    for file, old in base.items():
        new = (LIB / file).read_text(encoding='utf-8')
        if not new.endswith('\n'):
            # difflib cannot express a missing final newline in a form `git apply` accepts
            sys.exit(f'Lib/{file} must end with a newline')
        hunks.extend(difflib.unified_diff(old.splitlines(True), new.splitlines(True), f'a/{file}', f'b/{file}'))
    if not hunks:
        sys.exit('Lib/ has no edits beyond the existing patches')
    PATCHES.mkdir(exist_ok=True)
    target.write_text(f'{reason.strip()}\n\n{"".join(hunks)}', encoding='utf-8', newline='\n')
    print(f'wrote {target.relative_to(REPO)}; document any behaviour it changes under limitations/')


def render_table(tag: str, files: list[str]) -> str:
    """The Rust table of vendored sources, one entry per vendored file, formatted as `make format-rs` would."""
    entries = ''.join(
        f'    VendoredSource {{\n'
        f'        name: "{module_name(file)}",\n'
        f'        file: "{file}",\n'
        f'        source: include_str!("../../../stdlib/Lib/{file}"),\n'
        f'    }},\n'
        for file in files
    )
    return rustfmt(
        f'//! Generated by `scripts/vendor_stdlib.py` from `crates/monty/stdlib/vendor.toml`; do not edit.\n'
        f'\n'
        f'use super::VendoredSource;\n'
        f'\n'
        f'/// Every vendored module, copied from CPython {tag} and sorted by file path.\n'
        f'pub(super) const SOURCES: &[VendoredSource] = &[\n'
        f'{entries}'
        f'];\n'
    )


def rustfmt(source: str) -> str:
    """`source` formatted with the repository's rustfmt settings, so `make format-rs` leaves it unchanged."""
    result = subprocess.run(
        ['rustfmt', '+nightly', '--edition', '2024', '--config-path', REPO / '.rustfmt.toml', '--emit', 'stdout'],
        input=source,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode:
        sys.exit(f'rustfmt failed:\n{result.stderr}')
    return result.stdout


def module_name(file: str) -> str:
    """The dotted module name a `Lib/`-relative path defines: `pkg/__init__.py` defines `pkg`."""
    parts = file.removesuffix('.py').split('/')
    if parts[-1] == '__init__':
        parts.pop()
    return '.'.join(parts)


if __name__ == '__main__':
    main()
