"""Check that the vendored typeshed tree matches what `update.py` would produce.

`build.rs` zips `vendor/typeshed/` into the binary and that zip is the only
thing the type checker ever sees, but nothing regenerates it automatically —
editing `custom/*.pyi`, `COPY_FILES`, `VERSIONS` or `COMMIT` has no effect
until `update.py` runs. Drift fails silently rather than loudly: a member the
stale stub does not describe is reported as `unresolved-attribute`, and a
module missing from `VERSIONS` is treated as not existing at all.

Everything except `builtins.pyi` (whose filtering needs the upstream clone) is
verified offline against the constants in `update.py`.

Usage:
    python crates/monty-typeshed/check.py
"""

import sys
from pathlib import Path

# `update.py` is this script's sibling, so the interpreter's script directory
# resolves it. It owns the definition of what the vendored tree should hold, so
# importing it keeps the two from drifting the way the tree itself can.
import update

# Filtered from the upstream clone, so its contents cannot be re-derived
# offline — only its presence is checked.
GENERATED_FROM_UPSTREAM = 'stdlib/builtins.pyi'


def main() -> int:
    problems = [*check_tree_contents(), *check_custom_stubs(), *check_generated_files(), *check_versions()]
    if problems:
        print('vendored typeshed is out of sync:', file=sys.stderr)
        for problem in problems:
            print(f'  {problem}', file=sys.stderr)
        print('\nrun `make update-typeshed` to regenerate the vendored tree', file=sys.stderr)
        return 1
    else:
        print(f'vendored typeshed in sync ({len(expected_files())} files)')
        return 0


def check_tree_contents() -> list[str]:
    """The tree must hold exactly the files `update.py` writes.

    Comparing both directions is what catches a renamed or deleted stub, whose
    stale vendored copy would otherwise keep shipping in the zip unnoticed.
    """
    actual = {p.relative_to(update.VENDOR_DIR).as_posix() for p in update.VENDOR_DIR.rglob('*') if p.is_file()}
    expected = expected_files()
    return [
        *(f'missing, update.py would write it: {name}' for name in sorted(expected - actual)),
        *(f'stale, update.py no longer writes it: {name}' for name in sorted(actual - expected)),
    ]


def expected_files() -> set[str]:
    """Vendor-relative paths of every file `update.py` writes."""
    return {
        'source_commit.txt',
        'stdlib/VERSIONS',
        GENERATED_FROM_UPSTREAM,
        *(f'stdlib/{name}' for name in update.COPY_FILES),
        *(f'stdlib/{stub.name}' for stub in custom_stubs()),
    }


def custom_stubs() -> list[Path]:
    """The `custom/*.pyi` overrides, in a stable order."""
    return sorted(update.CUSTOM_DIR.glob('*.pyi'))


def check_custom_stubs() -> list[str]:
    """Each override must be byte-identical to the copy that gets zipped.

    Absent copies are left to `check_tree_contents` so they report once.
    """
    return [
        f'custom/{stub.name} differs from stdlib/{stub.name}'
        for stub in custom_stubs()
        if (vendored := read_vendored(f'stdlib/{stub.name}')) is not None and vendored != stub.read_bytes()
    ]


def check_generated_files() -> list[str]:
    """`source_commit.txt` and `VERSIONS` are written straight from constants,
    so editing a constant alone leaves the tree describing the previous state.
    """
    generated = (
        ('source_commit.txt', 'COMMIT', f'{update.COMMIT}\n'.encode()),
        ('stdlib/VERSIONS', 'VERSIONS', update.VERSIONS.encode()),
    )
    return [
        f'{name} does not match {constant} in update.py'
        for name, constant, expected in generated
        if (vendored := read_vendored(name)) is not None and vendored != expected
    ]


def check_versions() -> list[str]:
    """`stdlib/VERSIONS` gates module resolution: the type checker reports an
    unresolved import for a listed module with no stub, and ignores a stub
    whose module is unlisted.

    Upstream stubs vendored only as internal dependencies of another stub (e.g.
    `enum`, reached from `dataclasses`) are deliberately unlisted, so the
    listing is only required for `custom/` — the modules monty itself exposes.
    """
    listed = parse_versions(update.VERSIONS)
    return [
        *(
            f'{module} is listed in VERSIONS but has no vendored stub'
            for module in sorted(listed)
            if not resolves(module)
        ),
        *(
            f'custom/{stub.name} is missing from VERSIONS, so the type checker ignores it'
            for stub in custom_stubs()
            if stub.stem not in listed
        ),
    ]


def parse_versions(versions: str) -> set[str]:
    """Module names out of a typeshed `VERSIONS` file (`name: 3.0-  # note`)."""
    lines = (line.split('#')[0].strip() for line in versions.splitlines())
    return {line.split(':')[0].strip() for line in lines if line}


def resolves(module: str) -> bool:
    """Whether a (possibly dotted) module name has a vendored stub."""
    stem = update.STDLIB_DIR.joinpath(*module.split('.'))
    return stem.with_suffix('.pyi').is_file() or (stem / '__init__.pyi').is_file()


def read_vendored(name: str) -> bytes | None:
    """Contents of a vendor-relative file, or `None` when it does not exist."""
    path = update.VENDOR_DIR / name
    return path.read_bytes() if path.is_file() else None


if __name__ == '__main__':
    sys.exit(main())
