#!/usr/bin/env python3
"""Update vendored typeshed files from the upstream repository.

This script:
1. Clones the typeshed repository to a temporary directory
2. Records the HEAD commit hash
3. Filters builtins.pyi to keep only supported classes and functions
4. Writes the filtered file to the vendor directory

Usage:
    python crates/monty-typeshed/update.py
"""

import ast
import subprocess
import tempfile
from pathlib import Path

# Whitelisted builtin functions (from crates/monty/src/builtins/)
ALLOWED_FUNCTIONS = frozenset(
    {
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
)

# Whitelisted builtin classes (from crates/monty/src/types/ and exception_private.rs)
ALLOWED_CLASSES = frozenset(
    {
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
        'TimeoutError',
        'TypeError',
        'ValueError',
        'StopIteration',
    }
)

SCRIPT_DIR = Path(__file__).parent
VENDOR_DIR = SCRIPT_DIR / 'vendor' / 'typeshed'
STDLIB_DIR = VENDOR_DIR / 'stdlib'

TYPESHED_REPO = 'git@github.com:python/typeshed.git'


def clone_typeshed(temp_dir: Path) -> tuple[Path, str]:
    """Clone typeshed repository and return the path and HEAD commit hash.

    Args:
        temp_dir: Temporary directory to clone into.

    Returns:
        Tuple of (repo_path, commit_hash).
    """
    repo_path = temp_dir / 'typeshed'

    subprocess.run(
        ['git', 'clone', '--depth=1', TYPESHED_REPO, str(repo_path)],
        check=True,
        capture_output=True,
    )

    result = subprocess.run(
        ['git', 'rev-parse', 'HEAD'],
        cwd=repo_path,
        check=True,
        capture_output=True,
        text=True,
    )
    commit = result.stdout.strip()

    return repo_path, commit


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
            if node.name in ALLOWED_CLASSES:
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
    print(f'Cloning {TYPESHED_REPO}...')

    with tempfile.TemporaryDirectory() as temp_dir:
        temp_path = Path(temp_dir)

        # Clone and get commit
        repo_path, commit = clone_typeshed(temp_path)
        print(f'Cloned at commit {commit}')

        # Read source file
        builtins_path = repo_path / 'stdlib' / 'builtins.pyi'
        source = builtins_path.read_text()
        print(f'Read {len(source)} bytes from builtins.pyi')

        # Filter
        filtered = filter_builtins(source)
        print(f'Filtered to {len(filtered)} bytes')

        # Copy VERSIONS file
        versions_source = repo_path / 'stdlib' / 'VERSIONS'
        versions_content = versions_source.read_text()

        # Write output files
        STDLIB_DIR.mkdir(parents=True, exist_ok=True)
        (STDLIB_DIR / 'builtins.pyi').write_text(filtered)
        (STDLIB_DIR / 'VERSIONS').write_text(versions_content)
        (VENDOR_DIR / 'source_commit.txt').write_text(commit + '\n')

        print(f'Updated to commit {commit}')
        print(f'Wrote {STDLIB_DIR / "builtins.pyi"}')
        print(f'Wrote {STDLIB_DIR / "VERSIONS"}')
        print(f'Wrote {VENDOR_DIR / "source_commit.txt"}')

    return 0


if __name__ == '__main__':
    raise SystemExit(main())
