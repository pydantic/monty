"""
Vendor the CPython stdlib modules listed in `crates/monty/stdlib/vendor.toml`.

Downloads each listed file from `Lib/` at the pinned CPython commit, applies `crates/monty/stdlib/patches/*.patch` in
file-name order, and writes the result to `crates/monty/stdlib/Lib/`, along with CPython's `LICENSE`.
It also generates `crates/monty/src/modules/vendored/sources.rs`, the table the interpreter imports them from.

- `make vendor-stdlib`: write the runtime sources.
- `make check-stdlib` (`--check`): fail instead of writing if the runtime sources differ.
- `--prepare-tests`: extract the pinned `Lib/test` tree and apply the test-only patches to a local working copy.
- `--make-patch NAME --reason TEXT`: save the edits made to `Lib/` since the last vendoring as `patches/NAME.patch`,
    replacing a patch of that name. Add `--test` to work on the generated test tree instead.
- `make check-stdlib-patches` (`--check-patches`): fail if a patched module passes its test case unpatched, so its
    patches can be dropped. A patch whose reason has a `Permanent: <why>` line is kept without checking.
"""

import argparse
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.request
from collections import Counter
from pathlib import Path

from vendor_common import (
    REPO,
    PatchDoesNotApply,
    PatchError,
    apply_patches,
    check_patch,
    fetch,
    fetch_sources,
    git_apply,
    make_patch,
    patched_files,
    sync_tree,
)

STDLIB = REPO / 'crates' / 'monty' / 'stdlib'
MANIFEST = STDLIB / 'vendor.toml'
LIB = STDLIB / 'Lib'
PATCHES = STDLIB / 'patches'
TEST_VENDOR = REPO / 'crates' / 'monty-datatest' / 'vendor' / 'cpython'
TEST_WORK = TEST_VENDOR / 'work'
TEST_TREE = TEST_WORK / 'generated'
TEST_PATCHES = TEST_VENDOR / 'patches'
LICENSE = STDLIB / 'LICENSE'
TABLE = REPO / 'crates' / 'monty' / 'src' / 'modules' / 'vendored' / 'sources.rs'
TEST_CASES = REPO / 'crates' / 'monty' / 'test_cases'
UPSTREAM = 'python/cpython'
ARCHIVE_URL = f'https://codeload.github.com/{UPSTREAM}/tar.gz/{{commit}}'


def main() -> None:
    parser = argparse.ArgumentParser(description='Vendor CPython stdlib modules into crates/monty/stdlib.')
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--check', action='store_true', help='fail if the vendored files are out of date')
    mode.add_argument('--make-patch', metavar='NAME', help='save edits to Lib/ as patches/NAME.patch')
    mode.add_argument('--check-patches', action='store_true', help='fail if a patched module works unpatched')
    mode.add_argument(
        '--prepare-tests', action='store_true', help='regenerate the test-only tree from the pinned archive'
    )
    parser.add_argument('--reason', help='why the patch is needed; required with --make-patch')
    parser.add_argument('--test', action='store_true', help='save a patch to the test-only tree with --make-patch')
    args = parser.parse_args()
    if args.test and not args.make_patch:
        parser.error('--test requires --make-patch')
    if args.make_patch and not (args.reason or '').strip():
        parser.error('--make-patch needs a --reason explaining why the CPython source must change')

    manifest = tomllib.loads(MANIFEST.read_text(encoding='utf-8'))
    tag: str = manifest['tag']
    commit: str = manifest['commit']
    files: list[str] = sorted(manifest['files'])
    if args.prepare_tests or args.test:
        suites: list[str] = manifest['tests']['suites']
        support: list[str] = manifest['tests']['support']
        test_files = [f'test/{suite}' for suite in suites] + support
        if args.prepare_tests:
            prepare_tests(commit, test_files)
        else:
            if not (TEST_TREE / 'Lib').is_dir():
                sys.exit('Run `make prepare-cpython-tests` before editing the generated sources.')
            with tarfile.open(test_archive(commit), 'r:gz') as archive:
                sources = {file: read_archive_source(archive, f'cpython-{commit}/Lib/{file}') for file in test_files}
            make_patch(args.make_patch, args.reason, sources, TEST_TREE / 'Lib', TEST_PATCHES)
        return

    if args.make_patch:
        sources = fetch_sources(UPSTREAM, commit, 'Lib', files)
        make_patch(args.make_patch, args.reason, sources, LIB, PATCHES)
        print('document any behaviour it changes under limitations/')
        return

    upstream = fetch_sources(UPSTREAM, commit, 'Lib', files)
    if args.check_patches:
        check_patches(upstream, sorted(PATCHES.glob('*.patch')))
        return

    license_text = fetch(UPSTREAM, commit, 'LICENSE')
    expected = {LICENSE: license_text, TABLE: render_table(tag, files)}
    expected |= patched_tree(STDLIB, upstream)
    sync_tree(expected, (LIB,), check=args.check, hint='vendored stdlib is out of date, run `make vendor-stdlib`')
    if args.check:
        print(f'vendored stdlib matches CPython {tag}')
    else:
        print(f'vendored {len(files)} runtime modules from CPython {tag}')


def prepare_tests(commit: str, files: list[str]) -> None:
    """Stage all of Lib/test, but only let patches edit suites and support files listed in the manifest."""
    root = f'cpython-{commit}'
    selected = {f'{root}/Lib/{file}' for file in files} | {f'{root}/LICENSE'}
    archive_path = test_archive(commit)
    with tempfile.TemporaryDirectory(dir=TEST_WORK) as tmp, tarfile.open(archive_path, 'r:gz') as archive:
        members = [
            member
            for member in archive.getmembers()
            if member.name.startswith(f'{root}/Lib/test/') or member.name in selected
        ]
        archive.extractall(tmp, members=members, filter='data')
        source = Path(tmp) / root
        lib = source / 'Lib'
        patches = sorted(TEST_PATCHES.glob('*.patch'))
        for file in files:
            if not (lib / file).is_file():
                sys.exit(f'CPython {commit} archive is missing Lib/{file}')
        allowed = set(files)
        for patch in patches:
            check_patch(patch, allowed)
            git_apply(patch, lib)
        if TEST_TREE.exists():
            shutil.rmtree(TEST_TREE)
        source.rename(TEST_TREE)
    print(f'prepared CPython tests from {commit} in {TEST_TREE.relative_to(REPO)}')


def test_archive(commit: str) -> Path:
    """Cache the full upstream archive by commit, never a partially downloaded file."""
    cache = TEST_WORK / 'cache'
    cache.mkdir(parents=True, exist_ok=True)
    path = cache / f'{commit}.tar.gz'
    if not path.is_file():
        print(f'downloading CPython archive for {commit}', flush=True)
        with tempfile.TemporaryDirectory(dir=cache) as tmp:
            download = Path(tmp) / path.name
            with (
                urllib.request.urlopen(ARCHIVE_URL.format(commit=commit), timeout=60) as response,
                download.open('wb') as f,
            ):
                shutil.copyfileobj(response, f)
            download.replace(path)
    return path


def read_archive_source(archive: tarfile.TarFile, name: str) -> str:
    source = archive.extractfile(name)
    if source is None:
        sys.exit(f'CPython archive does not contain a regular file at {name}')
    with source:
        return source.read().decode('utf-8')


def patched_tree(root: Path, sources: dict[str, str]) -> dict[Path, str]:
    """Map a source tree's patched contents to their destination paths."""
    return {
        root / 'Lib' / file: text
        for file, text in apply_patches(sources, sorted((root / 'patches').glob('*.patch'))).items()
    }


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
