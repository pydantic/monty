"""Check the licence texts that ship with the CPython stdlib and typeshed."""

import argparse
import sys
import tarfile
import tomllib
import zipfile
from email import policy
from email.parser import BytesParser
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MONTY = ROOT / 'crates/monty'
STDLIB = MONTY / 'stdlib'
TYPESHED = ROOT / 'crates/monty-typeshed'
RUNTIME = ROOT / 'crates/monty-runtime'
RUNTIME_EXPRESSION = 'MIT AND Python-2.0.1 AND Apache-2.0'


def main() -> None:
    """Regenerate licence bundles or verify source copies and release archives."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--write', action='store_true', help='regenerate the licences used by release packages')
    parser.add_argument('--crates', type=Path, help='check .crate archives in this directory')
    parser.add_argument('--wheels', type=Path, nargs='+', default=[], help='check the runtime wheel archives')
    parser.add_argument('--sdist', type=Path, help='check the runtime source archive')
    args = parser.parse_args()

    mit = (ROOT / 'LICENSE').read_bytes()
    runtime = b'\n'.join(
        (
            mit,
            b'=== CPython standard library ===\n',
            (STDLIB / 'NOTICE').read_bytes(),
            (STDLIB / 'LICENSE').read_bytes(),
            b'=== typeshed ===\n',
            b'Monty uses a filtered and adapted subset of the typeshed standard library stubs.\n',
            (TYPESHED / 'vendor/typeshed/LICENSE').read_bytes(),
        )
    )
    generated = {
        MONTY / 'LICENSE': mit,
        TYPESHED / 'LICENSE': mit,
        RUNTIME / 'LICENSE': runtime,
    }
    for path, expected in generated.items():
        if args.write:
            path.write_bytes(expected)
        elif not path.is_file() or path.read_bytes() != expected:
            sys.exit(f'{path.relative_to(ROOT)} is out of date; run `make update-licenses`')

    if args.crates:
        verify_crates(args.crates)
    for wheel in args.wheels:
        verify_wheel(wheel, runtime)
    if args.sdist:
        verify_runtime_sdist(args.sdist, runtime)
    print('vendored licence files are in sync')


def verify_crates(directory: Path) -> None:
    """Check that source archives contain the original notices, not only an SPDX expression."""
    workspace = tomllib.loads((ROOT / 'Cargo.toml').read_text())
    version = workspace['workspace']['package']['version']
    notices = {
        'monty': ('LICENSE', 'stdlib/LICENSE', 'stdlib/NOTICE'),
        'monty-typeshed': ('LICENSE', 'vendor/typeshed/LICENSE'),
        'monty-runtime': ('LICENSE',),
    }
    for crate, names in notices.items():
        archive = directory / f'{crate}-{version}.crate'
        with tarfile.open(archive) as package:
            for name in names:
                verify_tar_license(package, f'{crate}-{version}/{name}', (ROOT / 'crates' / crate / name).read_bytes())


def verify_runtime_sdist(archive: Path, expected: bytes) -> None:
    """Verify the published licence and the copy used to build a wheel from the sdist."""
    prefix = archive.name.removesuffix('.tar.gz')
    with tarfile.open(archive) as package:
        verify_tar_license(package, f'{prefix}/LICENSE', expected)
        verify_tar_license(package, f'{prefix}/crates/monty-runtime/LICENSE', expected)


def verify_tar_license(package: tarfile.TarFile, path: str, expected: bytes) -> None:
    """Compare an archive member against the published licence source without extracting it."""
    try:
        source = package.extractfile(path)
    except KeyError:
        source = None
    if source is None:
        sys.exit(f'{package.name}: missing licence file {path}')
    with source:
        if source.read() != expected:
            sys.exit(f'{package.name}: {path} does not match the source')


def verify_wheel(wheel: Path, expected: bytes) -> None:
    """Check PEP 639 metadata and the accompanying licence bundle in a worker wheel."""
    with zipfile.ZipFile(wheel) as package:
        (metadata_path,) = (name for name in package.namelist() if name.endswith('.dist-info/METADATA'))
        metadata = BytesParser(policy=policy.default).parsebytes(package.read(metadata_path), headersonly=True)
        if metadata['License-Expression'] != RUNTIME_EXPRESSION:
            sys.exit(f'{wheel.name}: expected License-Expression: {RUNTIME_EXPRESSION}')
        if metadata.get_all('License-File') != ['LICENSE']:
            sys.exit(f'{wheel.name}: expected License-File: LICENSE')
        if package.read(f'{Path(metadata_path).parent.as_posix()}/licenses/LICENSE') != expected:
            sys.exit(f'{wheel.name}: LICENSE does not match the source')


if __name__ == '__main__':
    main()
