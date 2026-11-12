"""
Report which of CPython's pure-Python stdlib modules Monty can load, the first blocker of each that it cannot, and
every syntax blocker in each.

Each module's source is read from the running CPython (which must be 3.14, the version Monty targets) and run as a
script by the `monty` binary, so the report reflects the binary's language support, not an import path.
"Loads" only means the module body runs: calling its functions may still hit missing features.
Monty stops at the first error, so the script also scans each module's source with `ast` for every construct Monty's
parser rejects (see `SyntaxScan`), to show which modules a single language feature would unblock.
Run with `make stdlib-candidates`, or `uv run scripts/stdlib_candidates.py [--monty PATH] [MODULE ...]`.
"""

import argparse
import ast
import os
import subprocess
import sys
import sysconfig
import tempfile
from collections import Counter, defaultdict
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
        sys.exit(f'{args.monty} not found; build it with `cargo build --release -p monty-runtime --bin monty`')

    lib = Path(sysconfig.get_path('stdlib'))
    modules = args.modules or CANDIDATES
    by_outcome: dict[str, list[str]] = defaultdict(list)
    for module in modules:
        by_outcome[run_module(module, lib, args.monty, args.timeout)].append(module)

    print('First blocker (running the module in Monty):\n')
    loads = by_outcome.pop(LOADS, [])
    print(f'{LOADS} ({len(loads)}): {", ".join(loads) or "-"}')
    for outcome, blocked in sorted(by_outcome.items(), key=lambda item: (-len(item[1]), item[0])):
        print(f'\n{outcome} ({len(blocked)}):\n    {", ".join(blocked)}')

    print_syntax_blockers({module: syntax_blockers(module, lib) for module in modules})


def print_syntax_blockers(blockers: dict[str, Counter[str]]) -> None:
    """Print every syntax blocker with the modules containing it, then the modules a single feature would unblock.

    "Unblocks" only means the parser would accept the module's own file: its imports and runtime gaps may still fail.
    """
    by_feature: dict[str, list[str]] = defaultdict(list)
    for module, features in blockers.items():
        for feature, count in features.items():
            by_feature[feature].append(f'{module} ({count})')
    print('\n\nAll syntax blockers (an `ast` scan of each module; occurrences in parentheses):')
    for feature, modules in sorted(by_feature.items(), key=lambda item: (-len(item[1]), item[0])):
        print(f'\n{feature} ({len(modules)}):\n    {", ".join(modules)}')

    only: dict[str, list[str]] = defaultdict(list)
    for module, features in blockers.items():
        if len(features) == 1:
            only[next(iter(features))].append(module)
    clean = [module for module, features in blockers.items() if not features]
    print("\n\nSyntax blockers that are a module's only one:")
    for feature, modules in sorted(only.items(), key=lambda item: (-len(item[1]), item[0])):
        print(f'\n{feature} ({len(modules)}):\n    {", ".join(modules)}')
    print(f'\nno syntax blocker ({len(clean)}): {", ".join(clean) or "-"}')


def default_monty() -> Path:
    """The release `monty` binary under the cargo target directory.

    Not the debug one: debug builds cap source nesting at 30 levels (release: 200), which real stdlib code exceeds.
    """
    target = Path(os.environ.get('CARGO_TARGET_DIR', REPO / 'target'))
    return target / 'release' / ('monty.exe' if sys.platform == 'win32' else 'monty')


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


def syntax_blockers(module: str, lib: Path) -> Counter[str]:
    """Every construct in `module`'s own source that Monty's parser rejects, with how often each occurs."""
    source = module_source(module, lib)
    scan = SyntaxScan()
    if source is not None:
        scan.visit(ast.parse(source))
    return scan.blockers


class SyntaxScan(ast.NodeVisitor):
    """Counts the constructs Monty rejects before running any code, named by the error Monty raises for each.

    The rules mirror the `not_implemented` rejections in `crates/monty/src/parse.rs` (plus star and relative imports,
    rejected there with other errors): update them when Monty gains a feature, or the report overstates the work left.
    Rare rejections (unsupported `__future__` features, walrus in class bodies, class members shadowing a captured
    variable) are not checked.
    """

    def __init__(self) -> None:
        self.blockers: Counter[str] = Counter()

    def visit_Delete(self, node: ast.Delete) -> None:
        self.blockers["the 'del' statement"] += 1
        self.generic_visit(node)

    def visit_TypeAlias(self, node: ast.TypeAlias) -> None:
        self.blockers['type aliases'] += 1
        self.generic_visit(node)

    def visit_AsyncFor(self, node: ast.AsyncFor) -> None:
        self.blockers['async for loops'] += 1
        self.generic_visit(node)

    def visit_AsyncWith(self, node: ast.AsyncWith) -> None:
        self.blockers['async context managers (async with)'] += 1
        self.generic_visit(node)

    def visit_TryStar(self, node: ast.TryStar) -> None:
        self.blockers['exception groups (try*/except*)'] += 1
        self.generic_visit(node)

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        if node.level:
            self.blockers['relative imports'] += 1
        if any(alias.name == '*' for alias in node.names):
            self.blockers['wildcard imports (`from ... import *`)'] += 1

    def visit_Yield(self, node: ast.Yield) -> None:
        self.blockers['yield expressions'] += 1
        self.generic_visit(node)

    def visit_YieldFrom(self, node: ast.YieldFrom) -> None:
        self.blockers['yield from expressions'] += 1
        self.generic_visit(node)

    def visit_TemplateStr(self, node: ast.TemplateStr) -> None:
        self.blockers['template strings (t-strings)'] += 1
        self.generic_visit(node)

    def visit_comprehension(self, node: ast.comprehension) -> None:
        if node.is_async:
            self.blockers['async comprehensions'] += 1
        self.generic_visit(node)

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        if node.bases or node.keywords:
            self.blockers['class inheritance and metaclasses'] += 1
        for stmt in node.body:
            if blocker := class_body_blocker(stmt):
                self.blockers[blocker] += 1
        self.generic_visit(node)


def class_body_blocker(stmt: ast.stmt) -> str | None:
    """The feature a statement directly in a class body needs, or `None` if Monty accepts it there."""
    match stmt:
        case ast.FunctionDef() | ast.AsyncFunctionDef():
            return 'method decorators (classmethod/staticmethod/property)' if stmt.decorator_list else None
        case ast.Assign(targets=[ast.Name()]) | ast.AnnAssign(target=ast.Name()) | ast.AnnAssign(value=None):
            return None
        case ast.Assign() | ast.AnnAssign():
            return 'complex class variable targets'
        case ast.Pass():
            return None
        # a docstring, a later bare string or the `...` stub body
        case ast.Expr(value=ast.Constant(value=value)) if isinstance(value, str) or value is Ellipsis:
            return None
        case _:
            return 'class bodies containing anything other than methods and simple class variables'


def module_source(module: str, lib: Path) -> str | None:
    """The source of `module` (a plain module or a package's `__init__.py`), or `None` if it has none."""
    base = lib.joinpath(*module.split('.'))
    for path in (base.with_suffix('.py'), base / '__init__.py'):
        if path.is_file():
            return path.read_text(encoding='utf-8')
    return None


if __name__ == '__main__':
    main()
