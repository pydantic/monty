"""
Vendor the CPython stdlib modules listed in `crates/monty/stdlib/vendor.toml`.

Downloads each listed file from `Lib/` at the pinned CPython commit, applies `crates/monty/stdlib/patches/*.patch` in
file-name order, and writes the result to `crates/monty/stdlib/Lib/`, along with CPython's `LICENSE`.
It also generates `crates/monty/src/modules/vendored/sources.rs`, the table the interpreter imports them from.
Test-only sources use the same pin, but are written under `crates/monty-datatest/vendor/cpython/Lib/`.
The manifest can select individual definitions from test-only dependencies before their patches apply.

- `make vendor-stdlib`: write everything.
- `make check-stdlib` (`--check`): fail instead of writing if anything differs.
- `--make-patch NAME --reason TEXT`: save the edits made to `Lib/` since the last vendoring as `patches/NAME.patch`,
    replacing a patch of that name. Add `--test` to work on the test-only tree instead.
- `make check-stdlib-patches` (`--check-patches`): fail if a patched module passes its test case unpatched, so its
    patches can be dropped. A patch whose reason has a `Permanent: <why>` line is kept without checking.
"""

import argparse
import ast
import difflib
import os
import re
import subprocess
import sys
import tempfile
import tomllib
import urllib.request
from collections import Counter
from pathlib import Path

REPO = Path(__file__).parent.parent
STDLIB = REPO / 'crates' / 'monty' / 'stdlib'
MANIFEST = STDLIB / 'vendor.toml'
LIB = STDLIB / 'Lib'
PATCHES = STDLIB / 'patches'
TEST_VENDOR = REPO / 'crates' / 'monty-datatest' / 'vendor' / 'cpython'
LICENSE = STDLIB / 'LICENSE'
TABLE = REPO / 'crates' / 'monty' / 'src' / 'modules' / 'vendored' / 'sources.rs'
TEST_CASES = REPO / 'crates' / 'monty' / 'test_cases'
RAW_URL = 'https://raw.githubusercontent.com/python/cpython/{commit}/{path}'


def main() -> None:
    parser = argparse.ArgumentParser(description='Vendor CPython stdlib modules into crates/monty/stdlib.')
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--check', action='store_true', help='fail if the vendored files are out of date')
    mode.add_argument('--make-patch', metavar='NAME', help='save edits to Lib/ as patches/NAME.patch')
    mode.add_argument('--check-patches', action='store_true', help='fail if a patched module works unpatched')
    parser.add_argument('--reason', help='why the patch is needed; required with --make-patch')
    parser.add_argument('--test', action='store_true', help='save a patch to the test-only tree with --make-patch')
    args = parser.parse_args()
    if args.test and not args.make_patch:
        parser.error('--test requires --make-patch')

    manifest = tomllib.loads(MANIFEST.read_text(encoding='utf-8'))
    tag: str = manifest['tag']
    commit: str = manifest['commit']
    files: list[str] = sorted(manifest['files'])
    if args.make_patch:
        if not (args.reason or '').strip():
            sys.exit('--make-patch needs a --reason explaining why the CPython source must change')
        sources = test_sources(commit, manifest['tests']) if args.test else fetch_sources(commit, files)
        make_patch(args.make_patch, args.reason, sources, TEST_VENDOR if args.test else STDLIB)
        return

    upstream = fetch_sources(commit, files)
    if args.check_patches:
        check_patches(upstream, sorted(PATCHES.glob('*.patch')))
        return

    license_text = fetch(commit, 'LICENSE')
    expected = {LICENSE: license_text, TEST_VENDOR / 'LICENSE': license_text, TABLE: render_table(tag, files)}
    expected |= patched_tree(STDLIB, upstream)
    expected |= patched_tree(TEST_VENDOR, test_sources(commit, manifest['tests']))
    stale = [
        path
        for root in (LIB, TEST_VENDOR / 'Lib')
        for path in root.rglob('*')
        if path.is_file() and path not in expected
    ]

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
        print(
            f'vendored {len(files)} runtime modules and {len(manifest["tests"]["files"])} test files from CPython {tag}'
        )


def fetch_sources(commit: str, files: list[str]) -> dict[str, str]:
    """Fetch each file by its path relative to `Lib/`."""
    return {file: fetch(commit, f'Lib/{file}') for file in files}


def test_sources(commit: str, config: dict[str, list[str] | dict[str, list[str]]]) -> dict[str, str]:
    """Fetch the test suite and retain only the requested definitions in its dependencies."""
    files = config['files']
    extracts = config['extract']
    assert isinstance(files, list) and isinstance(extracts, dict)
    sources = fetch_sources(commit, files)
    for file, names in extracts.items():
        source = sources[file]
        sources[file] = extract_definitions(source.splitlines(keepends=True), ast.parse(source).body, names)
    return sources


def extract_definitions(lines: list[str], body: list[ast.stmt], names: list[str]) -> str:
    """Copy selected statements verbatim; dotted selectors keep only the requested class members."""
    excerpts: list[str] = []
    for name, members in group_selectors(names).items():
        matches = [node for node in body if definition_name(node) == name]
        if len(matches) != 1:
            sys.exit(f'expected one upstream definition for {name}, found {len(matches)}')
        node = matches[0]
        if members:
            if not isinstance(node, ast.ClassDef) or node.decorator_list:
                sys.exit(f'cannot select members of {name}: expected an undecorated class')
            excerpts.append(''.join(lines[node.lineno - 1 : node.body[0].lineno - 1]))
            excerpts.append(extract_definitions(lines, node.body, members) + '\n')
        else:
            excerpts.append(''.join(lines[node.lineno - 1 : node.end_lineno]) + '\n')
    return ''.join(excerpts).rstrip() + '\n'


def group_selectors(names: list[str]) -> dict[str, list[str]]:
    """Group class members by their class, in manifest order."""
    groups: dict[str, list[str]] = {}
    for selector in names:
        name, _, member = selector.partition('.')
        members = groups.setdefault(name, [])
        if member:
            members.append(member)
    return groups


def definition_name(node: ast.stmt) -> str | None:
    """Identify the statements the test manifest can select from upstream."""
    match node:
        case ast.FunctionDef(name=name, decorator_list=[]) | ast.ClassDef(name=name):
            return name
        case ast.Assign(targets=[ast.Name(id=name)]):
            return name
        case ast.ImportFrom(names=[ast.alias(name=name, asname=None)]):
            return name
        case ast.Expr(value=ast.Constant(value=str())):
            return '__doc__'
        case _:
            return None


def patched_tree(root: Path, sources: dict[str, str]) -> dict[Path, str]:
    """Map a source tree's patched contents to their destination paths."""
    return {
        root / 'Lib' / file: text
        for file, text in apply_patches(sources, sorted((root / 'patches').glob('*.patch'))).items()
    }


def fetch(commit: str, path: str) -> str:
    """One file from the CPython repository at `commit`, which unlike a tag cannot be moved to other content."""
    with urllib.request.urlopen(RAW_URL.format(commit=commit, path=path), timeout=30) as response:
        return response.read().decode('utf-8')


def read(path: Path) -> str | None:
    """A file's exact text, or `None` if it does not exist; unlike `read_text()`, it keeps CRLF line endings."""
    return path.read_bytes().decode('utf-8') if path.is_file() else None


class PatchError(Exception):
    """A patch that is malformed or does not apply; `main` reports it and exits."""


class PatchDoesNotApply(PatchError):
    """A well-formed patch whose context is not in the files, e.g. because an earlier patch it builds on was dropped."""


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
        raise PatchError(f'{patch.relative_to(REPO)} must start with the reason it is needed')
    for line in text.splitlines():
        if line.startswith(('--- a/', '+++ b/')) and line[6:] not in sources:
            raise PatchError(f'{patch.relative_to(REPO)} edits {line[6:]}, which vendor.toml does not list')


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
        raise PatchDoesNotApply(f'{patch.relative_to(REPO)} does not apply:\n{result.stderr}')


def make_patch(name: str, reason: str, upstream: dict[str, str], root: Path) -> None:
    """Saves the difference between the patched upstream files and `Lib/` as `patches/<name>.patch`.

    The earlier patches are applied first, so the new one holds only the edits they do not already make.
    Only the last patch can be replaced: later ones were made against its result.
    """
    patch_dir = root / 'patches'
    target = patch_dir / f'{name}.patch'
    patches = sorted(patch_dir.glob('*.patch'))
    if later := [patch.name for patch in patches if patch.name > target.name]:
        sys.exit(f'{target.name} would sort before {", ".join(later)}; pick a name that sorts last')
    base = apply_patches(upstream, [patch for patch in patches if patch != target])
    hunks: list[str] = []
    for file, old in base.items():
        new = (root / 'Lib' / file).read_text(encoding='utf-8')
        if not new.endswith('\n'):
            # difflib cannot express a missing final newline in a form `git apply` accepts
            sys.exit(f'Lib/{file} must end with a newline')
        hunks.extend(difflib.unified_diff(old.splitlines(True), new.splitlines(True), f'a/{file}', f'b/{file}'))
    if not hunks:
        sys.exit('Lib/ has no edits beyond the existing patches')
    patch_dir.mkdir(exist_ok=True)
    target.write_text(f'{reason.strip()}\n\n{"".join(hunks)}', encoding='utf-8', newline='\n')
    print(f'wrote {target.relative_to(REPO)}')
    if root == STDLIB:
        print('document any behaviour it changes under limitations/')


def check_patches(upstream: dict[str, str], patches: list[Path]) -> None:
    """Fails if a patch is no longer needed: its modules' test cases pass without it.

    One build drops every patch not marked `Permanent:`. A patch sharing a file with another checked patch then gets
    its own build, dropping only it, so a stale patch is found even when the other is still needed. Each build writes
    `Lib/`, rebuilds the datatest binary and runs `<module>__module.py` on Monty and CPython; `Lib/` is restored after.
    A pass shows only what the test case covers, so it should exercise the code each patch changes.
    """
    checked = [patch for patch in patches if not is_permanent(patch)]
    files = sorted({file for patch in checked for file in patched_files(patch)})
    if not files:
        print('no patch to check')
        return
    tests = {file: TEST_CASES / f'{module_name(file).replace(".", "_")}__module.py' for file in files}
    if missing := [test for test in tests.values() if not test.is_file()]:
        sys.exit('patched modules need a test case:\n' + '\n'.join(f'    {test.relative_to(REPO)}' for test in missing))

    # every patch must be valid and apply in full, so a failure below can only come from a dropped patch
    apply_patches(upstream, patches)
    originals = {file: (LIB / file).read_bytes() for file in files}
    try:
        passing = run_without(upstream, patches, set(checked), files, tests)
        stale = [patch for patch in checked if patched_files(patch) <= passing]
        patches_per_file = Counter(file for patch in checked for file in patched_files(patch))
        for patch in checked:
            if patch in stale or all(patches_per_file[file] == 1 for file in patched_files(patch)):
                continue
            try:
                passing = run_without(upstream, patches, {patch}, files, tests, sorted(patched_files(patch)))
            except PatchDoesNotApply:
                continue  # a later patch only applies on top of this one, so it stays
            if patched_files(patch) <= passing:
                stale.append(patch)
    finally:
        for file, data in originals.items():
            (LIB / file).write_bytes(data)

    if stale:
        listed = '\n'.join(f'    {patch.name} ({", ".join(sorted(patched_files(patch)))})' for patch in stale)
        sys.exit(
            f'these patches are no longer needed: their modules pass their test cases without them. Drop them (and '
            f'any limitations/ entry), or mark one kept for another reason with a `Permanent: <why>` line:\n{listed}'
        )
    print(f'every patch is still needed: {", ".join(patch.name for patch in checked)}')


def run_without(
    upstream: dict[str, str],
    patches: list[Path],
    dropped: set[Path],
    files: list[str],
    tests: dict[str, Path],
    run: list[str] | None = None,
) -> set[str]:
    """Writes `files` to `Lib/` with every patch but `dropped` applied, rebuilds, and returns the ones of `run`
    (default: all of `files`) whose test case passes. Raises `PatchDoesNotApply` if the kept patches do not apply."""
    lib = apply_patches(upstream, [patch for patch in patches if patch not in dropped])
    for file in files:
        (LIB / file).write_text(lib[file], encoding='utf-8', newline='\n')
    # built first, so a build failure stops the check rather than reading as a failing test
    subprocess.run(['cargo', 'build', '-q', '-p', 'monty-datatest'], cwd=REPO, check=True)
    return {file for file in (files if run is None else run) if datatest_passes(tests[file].name)}


def is_permanent(patch: Path) -> bool:
    """Whether the patch's reason, the text before its first hunk, has a `Permanent:` line."""
    reason = patch.read_text(encoding='utf-8').split('\n--- ', 1)[0]
    return any(line.startswith('Permanent:') for line in reason.splitlines())


def patched_files(patch: Path) -> set[str]:
    """The `Lib/`-relative paths a patch edits."""
    return {line[6:] for line in patch.read_text(encoding='utf-8').splitlines() if line.startswith('+++ b/')}


def datatest_passes(test: str) -> bool:
    """Whether the datatest case named `test` passes on both Monty and CPython; fails if no case matched."""
    result = subprocess.run(
        ['cargo', 'run', '-q', '-p', 'monty-datatest', '--', f'::{test}'],
        cwd=REPO,
        capture_output=True,
        text=True,
        check=False,
    )
    ran = re.search(r'test result: \w+\. (\d+) passed; (\d+) failed', result.stdout)
    if ran is None or int(ran[1]) + int(ran[2]) == 0:
        sys.exit(f'no datatest case ran for {test}:\n{result.stdout}{result.stderr}')
    return result.returncode == 0


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
    try:
        main()
    except PatchError as error:
        sys.exit(str(error))
