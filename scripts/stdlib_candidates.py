"""
Report which of CPython's pure-Python stdlib modules Monty can load, and the first blocker of each that it cannot.

Each module's source is read from the running CPython (which must be 3.14, the version Monty targets) and run as a
script by the `monty` binary, so the report reflects the binary's language support, not an import path.
"Loads" only means the module body runs: calling its functions may still hit missing features.
Run with `make stdlib-candidates`, or `uv run scripts/stdlib_candidates.py [--monty PATH] [MODULE ...]`.
"""

import argparse
import os
import subprocess
import sys
import sysconfig
import tempfile
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).parent.parent
# The outcome reported for a module whose body runs to completion.
LOADS = 'loads'

# Pure-Python modules named in https://github.com/pydantic/monty/issues/1275 (§3-§6), plus their imports.
CANDIDATES = [
    # requested modules (§3)
    'statistics',
    'fractions',
    'numbers',
    'ipaddress',
    'secrets',
    'hmac',
    'urllib.parse',
    # modules to migrate from native to vendored (§4)
    'typing',
    'annotationlib',
    'dataclasses',
    'collections',
    '_collections_abc',
    'functools',
    'random',
    'copy',
    'fnmatch',
    'base64',
    'tomllib._parser',
    'tomllib._re',
    'tomllib._types',
    'csv',
    'posixpath',
    'genericpath',
    'pathlib',
    # other pure-Python modules (§5)
    '_py_abc',
    'abc',
    'types',
    'copyreg',
    'keyword',
    '_py_warnings',
    'warnings',
    'operator',
    'heapq',
    'bisect',
    'contextlib',
    'enum',
    'string',
    'textwrap',
    'difflib',
    'pprint',
    'reprlib',
    'shlex',
    'html',
    'html.entities',
    'uuid',
    'calendar',
    'graphlib',
    'colorsys',
    'logging',
    'configparser',
    'getopt',
    'stat',
    # `unittest` and its dependencies (§6)
    'unittest',
    'unittest.case',
    'unittest.result',
    'traceback',
    'linecache',
    'weakref',
    '_weakrefset',
]


def main() -> None:
    parser = argparse.ArgumentParser(description='Report which CPython stdlib modules Monty can load.')
    parser.add_argument('modules', nargs='*', help='dotted module names; defaults to the #1275 candidates')
    parser.add_argument('--monty', type=Path, default=default_monty(), help='the monty binary to run')
    parser.add_argument('--timeout', type=float, default=20, help='seconds allowed per module')
    args = parser.parse_args()

    if sys.implementation.name != 'cpython' or sys.version_info[:2] != (3, 14):
        sys.exit(f'run this with CPython 3.14, not {sys.implementation.name} {sys.version.split()[0]}')
    if not args.monty.is_file():
        sys.exit(f'{args.monty} not found; build it with `cargo build -p monty-runtime --bin monty`')

    lib = Path(sysconfig.get_path('stdlib'))
    by_outcome: dict[str, list[str]] = defaultdict(list)
    for module in args.modules or CANDIDATES:
        by_outcome[run_module(module, lib, args.monty, args.timeout)].append(module)

    loads = by_outcome.pop(LOADS, [])
    print(f'{LOADS} ({len(loads)}): {", ".join(loads) or "-"}')
    for outcome, modules in sorted(by_outcome.items(), key=lambda item: (-len(item[1]), item[0])):
        print(f'\n{outcome} ({len(modules)}):\n    {", ".join(modules)}')


def default_monty() -> Path:
    """The debug `monty` binary under the cargo target directory."""
    target = Path(os.environ.get('CARGO_TARGET_DIR', REPO / 'target'))
    return target / 'debug' / ('monty.exe' if sys.platform == 'win32' else 'monty')


def run_module(module: str, lib: Path, monty: Path, timeout: float) -> str:
    """Run one module's source in Monty and return `LOADS` or the last line of its error.

    The source runs through `exec()` with the module's own `__name__`, as an import would, so `__main__` blocks
    stay skipped; importing it by name instead would load Monty's vendored copy of a module that has one.
    Each module runs alone in an empty directory, so it cannot import a sibling candidate by accident.
    """
    source = module_source(module, lib)
    if source is None:
        return 'not found in this CPython'
    with tempfile.TemporaryDirectory() as tmp:
        driver = Path(tmp) / 'driver.py'
        driver.write_text(f'exec({source!r}, {{"__name__": {module!r}}})\n', encoding='utf-8')
        try:
            result = subprocess.run(
                [monty, driver.name],
                cwd=tmp,
                capture_output=True,
                text=True,
                encoding='utf-8',
                errors='replace',
                timeout=timeout,
                check=False,
            )
        except subprocess.TimeoutExpired:
            return f'timed out after {timeout:g}s'
    if result.returncode == 0:
        return LOADS
    # the error is the last line on stderr; stdout only holds what the module printed
    lines = [line for line in (result.stderr or result.stdout).splitlines() if line.strip()]
    return lines[-1].strip() if lines else f'exit code {result.returncode} with no output'


def module_source(module: str, lib: Path) -> str | None:
    """The source of `module` (a plain module or a package's `__init__.py`), or `None` if it has none."""
    base = lib.joinpath(*module.split('.'))
    for path in (base.with_suffix('.py'), base / '__init__.py'):
        if path.is_file():
            return path.read_text(encoding='utf-8')
    return None


if __name__ == '__main__':
    main()
