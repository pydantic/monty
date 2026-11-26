"""Vendor the stubs selected by `crates/monty-typeshed/vendor.toml` from python/typeshed."""

import argparse
import hashlib
import sys
import tomllib
from pathlib import Path

from vendor_common import REPO, PatchError, apply_patches, fetch, fetch_sources, make_patch, patched_files, sync_tree

CRATE = REPO / 'crates' / 'monty-typeshed'
VENDOR = CRATE / 'vendor' / 'typeshed'
STDLIB = VENDOR / 'stdlib'
PATCHES = CRATE / 'patches'
UPSTREAM = 'python/typeshed'


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--check', action='store_true', help='fail if the vendored files are out of date')
    mode.add_argument('--make-patch', metavar='NAME', help='save edits to the vendored stubs as patches/NAME.patch')
    parser.add_argument('--reason', help='why the patch is needed; required with --make-patch')
    args = parser.parse_args()
    if args.make_patch and not (args.reason or '').strip():
        parser.error('--make-patch requires a nonempty --reason')

    manifest = tomllib.loads((CRATE / 'vendor.toml').read_text(encoding='utf-8'))
    commit: str = manifest['commit']
    additions: tuple[str, ...] = tuple(manifest.get('additions', []))
    upstream = fetch_sources(UPSTREAM, commit, 'stdlib', manifest['files'])
    if args.make_patch:
        make_patch(args.make_patch, args.reason, upstream, STDLIB, PATCHES, additions=additions)
        return

    patches = sorted(PATCHES.glob('*.patch'))
    stubs = apply_patches(upstream, patches, additions=additions)
    if missing := set(additions) - stubs.keys():
        sys.exit(f'typeshed patches did not add declared files: {", ".join(sorted(missing))}')
    validate_versions(stubs, patches)
    license_text = fetch(UPSTREAM, commit, 'LICENSE')
    if hashlib.sha256(license_text.encode('utf-8')).hexdigest() != manifest['license_sha256']:
        sys.exit('the upstream typeshed LICENSE changed; review it and update license_sha256 in vendor.toml')

    expected = {VENDOR / 'LICENSE': license_text, VENDOR / 'source_commit.txt': f'{commit}\n'}
    expected.update({STDLIB / file: text for file, text in stubs.items()})
    sync_tree(
        expected, (VENDOR,), check=args.check, hint='vendored typeshed is out of date, run `make update-typeshed`'
    )
    if args.check:
        print(f'vendored typeshed matches {UPSTREAM} at {commit}')
    else:
        print(f'vendored {len(stubs) - 1} stubs from {UPSTREAM} at {commit}')


def validate_versions(stubs: dict[str, str], patches: list[Path]) -> None:
    """Every listed module needs a stub; patched stubs must be visible to the type checker."""
    listed = {
        entry.split(':', 1)[0].strip()
        for line in stubs['VERSIONS'].splitlines()
        if (entry := line.split('#', 1)[0].strip())
    }
    for module in sorted(listed):
        stem = module.replace('.', '/')
        if f'{stem}.pyi' not in stubs and f'{stem}/__init__.pyi' not in stubs:
            sys.exit(f'{module} is listed in typeshed VERSIONS but has no vendored stub')

    for file in sorted({file for patch in patches for file in patched_files(patch) if file.endswith('.pyi')}):
        parts = file.removesuffix('.pyi').removesuffix('/__init__').split('/')
        if not any('.'.join(parts[:end]) in listed for end in range(1, len(parts) + 1)):
            sys.exit(f'patched stub {file} is not listed in typeshed VERSIONS, so the type checker ignores it')


if __name__ == '__main__':
    try:
        main()
    except PatchError as error:
        sys.exit(str(error))
