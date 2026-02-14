#!/usr/bin/env python3
"""Update vendored typeshed files from the upstream repository.

This script:
1. Clones the typeshed repository to crates/monty-typeshed/typeshed-repo (or updates if it exists)
2. Records the HEAD commit hash
3. Filters builtins.pyi to keep only supported classes and functions
4. Writes the filtered file to the vendor directory

Usage:
    python crates/monty-typeshed/update.py
"""

import ast
import shutil
import subprocess
from pathlib import Path

# Whitelisted builtin functions (from crates/monty/src/builtins/)
ALLOWED_FUNCTIONS = {
    'abs',
    'all',
    'any',
    'bin',
    'chr',
    'divmod',
    'hash',
    'hex',
    'id',
    'isinstance',
    'len',
    'max',
    'min',
    'oct',
    'ord',
    'pow',
    'print',
    'repr',
    'round',
    'sorted',
    'sum',
}

# Whitelisted builtin classes (from crates/monty/src/types/ and exception_private.rs)
ALLOWED_CLASSES = {
    # Core types
    'object',
    'type',
    # Primitive types
    'bool',
    'int',
    'float',
    # String/bytes types
    'str',
    'bytes',
    # Container types
    'list',
    'tuple',
    'dict',
    'set',
    'frozenset',
    'range',
    # Iterator types (these are classes, not functions)
    'enumerate',
    'reversed',
    'zip',
    # Slicing
    'slice',
    # property is used by pathlib.Path
    'property',
    # Exception hierarchy (from crates/monty/src/exception_private.rs)
    'BaseException',
    'Exception',
    'SystemExit',
    'KeyboardInterrupt',
    'ArithmeticError',
    'OverflowError',
    'ZeroDivisionError',
    'LookupError',
    'IndexError',
    'KeyError',
    'RuntimeError',
    'NotImplementedError',
    'RecursionError',
    'AttributeError',
    'AssertionError',
    'MemoryError',
    'NameError',
    'SyntaxError',
    'OSError',
    'TimeoutError',
    'TypeError',
    'ValueError',
    'StopIteration',
}

# Files to copy without filtering
COPY_FILES = [
    # Core type system
    'typing.pyi',
    'typing_extensions.pyi',
    '_collections_abc.pyi',
    # Used in type annotations
    'types.pyi',
    # So type checking works with dataclasses
    'dataclasses.pyi',
    # used by dataclasses
    'enum.pyi',
    # ==============================
    # all all collections dir
    'collections/__init__.pyi',
    'collections/abc.pyi',
    # ==============================
    # Take only `__init__.pyi` from _typeshed dir
    '_typeshed/__init__.pyi',
    # ==============================
    # all of pathlib dir
    'pathlib/__init__.pyi',
    'pathlib/types.pyi',
]
# content for typeshed's `VERSIONS` file
VERSIONS = """\
# absolutely minimal VERSIONS file exposing only the modules required
# all these modules are required to get type checking working with ty
# or for the stdlib modules we (partially) implement

_collections_abc: 3.3-
_typeshed: 3.0-  # not present at runtime, only for type checking
asyncio: 3.4-
builtins: 3.0-
collections: 3.0-
dataclasses: 3.7-
os: 3.0-
pathlib: 3.4-
pathlib.types: 3.14-
sys: 3.0-
typing: 3.5-
typing_extensions: 3.7-
types: 3.0-
"""

CRATE_DIR = Path(__file__).parent
REPO_ROOT = CRATE_DIR.parent.parent
VENDOR_DIR = CRATE_DIR / 'vendor' / 'typeshed'
STDLIB_DIR = VENDOR_DIR / 'stdlib'
CUSTOM_DIR = CRATE_DIR / 'custom'
TYPESHED_REPO_DIR = CRATE_DIR / 'typeshed-repo'

TYPESHED_REPO_URL = 'git@github.com:python/typeshed.git'


def clone_or_update_typeshed() -> str:
    """Clone or update the typeshed repository and return the path and HEAD commit hash.

    If the repository already exists at TYPESHED_REPO_DIR, performs a git pull.
    Otherwise, clones the repository to that location.

    Returns:
        commit_hash
    """
    if TYPESHED_REPO_DIR.exists():
        print(f'{TYPESHED_REPO_DIR} exists, not pulling')
    else:
        print(f'Cloning typeshed to {TYPESHED_REPO_DIR}...')
        subprocess.run(
            ['git', 'clone', '--depth=1', TYPESHED_REPO_URL, str(TYPESHED_REPO_DIR)],
            check=True,
            capture_output=True,
        )

    result = subprocess.run(
        ['git', 'rev-parse', 'HEAD'],
        cwd=TYPESHED_REPO_DIR,
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def filter_statements(nodes: list[ast.stmt]) -> list[ast.stmt]:
    """Filter a list of statements to keep only allowed functions and classes.

    Keeps:
    - Imports
    - Type variable assignments (e.g., _T = TypeVar('_T'))
    - Allowed function definitions
    - Allowed class definitions

    Args:
        nodes: List of AST statement nodes.

    Returns:
        Filtered list of statements.
    """
    result: list[ast.stmt] = []
    for node in nodes:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            if node.name in ALLOWED_FUNCTIONS:
                result.append(node)
        elif isinstance(node, ast.ClassDef):
            if node.name.startswith('_') or node.name in ALLOWED_CLASSES:
                result.append(node)
        elif isinstance(node, ast.If):
            # Recursively filter version-conditional blocks
            filtered = filter_if_block(node)
            if filtered is not None:
                result.append(filtered)
        else:
            # Keep imports, type aliases, assignments, etc.
            result.append(node)
    return result


def filter_if_block(node: ast.If) -> ast.If | None:
    """Filter an if block, recursively filtering function and class definitions.

    Handles version conditionals like `if sys.version_info >= (3, 10):`.

    Args:
        node: An ast.If node.

    Returns:
        Filtered If node, or None if both branches are empty after filtering.
    """
    filtered_body = filter_statements(node.body)
    filtered_orelse = filter_statements(node.orelse)

    # If both branches are empty, skip this if block entirely
    if not filtered_body and not filtered_orelse:
        return None

    # Create a new If node with filtered contents
    new_node = ast.If(
        test=node.test,
        body=filtered_body if filtered_body else [ast.Pass()],
        orelse=filtered_orelse,
    )
    return ast.copy_location(new_node, node)


def filter_builtins(source: str) -> str:
    """Filter builtins.pyi to keep only allowed classes and functions.

    This function parses the source with Python's ast module and filters
    top-level definitions to only include those in the allow lists.
    All imports and type definitions are preserved.

    Args:
        source: The source code of builtins.pyi.

    Returns:
        Filtered source code.
    """
    tree = ast.parse(source)
    tree.body = filter_statements(tree.body)
    ast.fix_missing_locations(tree)
    return ast.unparse(tree)


def main() -> int:
    """Main entry point."""
    # Clean up any stale files from previous runs
    if VENDOR_DIR.exists():
        print(f'Removing existing {VENDOR_DIR}...')
        shutil.rmtree(VENDOR_DIR)

    # Clone or update typeshed
    commit = clone_or_update_typeshed()
    print(f'At python/typeshed commit {commit}')

    # Read source file
    src_stdlib = TYPESHED_REPO_DIR / 'stdlib'
    builtins_path = src_stdlib / 'builtins.pyi'
    source = builtins_path.read_text()

    # Filter
    filtered = filter_builtins(source)

    # Write output files
    STDLIB_DIR.mkdir(parents=True, exist_ok=True)

    (STDLIB_DIR / 'builtins.pyi').write_text(filtered)
    print(f'Wrote {(STDLIB_DIR / "builtins.pyi").relative_to(REPO_ROOT)}')

    (STDLIB_DIR / 'VERSIONS').write_text(VERSIONS)
    print(f'Wrote {(STDLIB_DIR / "VERSIONS").relative_to(REPO_ROOT)}')

    (VENDOR_DIR / 'source_commit.txt').write_text(commit + '\n')
    print(f'Wrote {(VENDOR_DIR / "source_commit.txt").relative_to(REPO_ROOT)}')

    for file_path in COPY_FILES:
        src_file = src_stdlib / file_path
        if src_file.exists():
            dest_file = STDLIB_DIR / file_path
            dest_file.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(src_file, dest_file)
        else:
            raise ValueError(f'{file_path} not found in typeshed')
    print(f'Copied {len(COPY_FILES)} stdlib typeshed files')

    # copy pyi files from CUSTOM_DIR into STDLIB_DIR
    custom_count = 0
    for file in CUSTOM_DIR.glob('*.pyi'):
        shutil.copy2(file, STDLIB_DIR)
        custom_count += 1
    print(f'Copied {custom_count} custom typeshed files')

    return 0


if __name__ == '__main__':
    exit(main())
