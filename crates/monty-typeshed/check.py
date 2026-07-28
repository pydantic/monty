"""Check that the vendored typeshed stubs match their `custom/` sources.

`build.rs` zips `vendor/typeshed/` into the binary, and `update.py` populates
that directory by copying `custom/*.pyi` over the upstream stubs. Editing a
file in `custom/` therefore has no effect until `update.py` runs — the type
checker keeps using the stale vendored copy, so new module members are
reported as `unresolved-attribute`.

Usage:
    python crates/monty-typeshed/check.py

Exits with code 1 (listing the drifted files) if any copy is out of date.
"""

import sys
from pathlib import Path

CRATE_DIR = Path(__file__).parent
CUSTOM_DIR = CRATE_DIR / 'custom'
STDLIB_DIR = CRATE_DIR / 'vendor' / 'typeshed' / 'stdlib'


def main() -> int:
    custom_files = sorted(CUSTOM_DIR.glob('*.pyi'))
    # A brand-new custom stub has no vendored copy at all, hence `is_file`.
    drifted = [
        f.name
        for f in custom_files
        if not (STDLIB_DIR / f.name).is_file() or (STDLIB_DIR / f.name).read_bytes() != f.read_bytes()
    ]

    if drifted:
        print('vendored typeshed stubs are out of sync with custom/:', file=sys.stderr)
        for name in drifted:
            print(f'  {name}', file=sys.stderr)
        print('\nrun `make update-typeshed` to regenerate them', file=sys.stderr)
        return 1
    else:
        print(f'{len(custom_files)} custom typeshed stubs in sync')
        return 0


if __name__ == '__main__':
    sys.exit(main())
