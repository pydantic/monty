"""OSAccess compatibility tests.

These tests verify that OSAccess (Monty's virtual filesystem) behaves identically
to CPython's real filesystem operations. Each test runs twice - once with Monty
using OSAccess/MemoryFile and once with CPython using a real temp directory.

This ensures that code written for real filesystems works correctly in the
sandboxed Monty environment.
"""

from abc import ABC, abstractmethod
from pathlib import Path
from typing import Any, TypeAlias

import pytest

from pydantic_monty import MemoryFile, Monty, OSAccess

# Type alias for nested tree structure (file content or nested dict).
# Using Any for the recursive dict value since Python's type system doesn't
# handle recursive types well without TypedDict or Protocol.
TreeDict: TypeAlias = 'dict[str, str | bytes | TreeDict]'


class CodeRunner(ABC):
    """Abstract interface for running Python code against a filesystem.

    Implementations provide either a virtual filesystem (Monty+OSAccess) or
    a real filesystem (CPython+temp directory) for compatibility testing.
    """

    @abstractmethod
    def write_file(self, path: str, content: str | bytes) -> None:
        """Add a file to the test filesystem setup.

        Args:
            path: Relative path for the file (e.g., 'test/file.txt')
            content: File content as string or bytes
        """

    @abstractmethod
    def run_code(self, code: str) -> Any:
        """Run Python code and return the result.

        The code can use Path('relative/path') and it will be resolved to the
        appropriate root (OSAccess root or temp directory).

        Args:
            code: Python code to execute

        Returns:
            The result of the last expression in the code

        Raises:
            Exception: If the code raises an exception
        """

    @abstractmethod
    def tree(self) -> TreeDict:
        """Return a dict tree of files and their contents.

        Returns:
            Nested dict where keys are file/dir names and values are:
            - str/bytes for file contents
            - dict for subdirectories
        """

    @abstractmethod
    def set_environ(self, environ: dict[str, str]) -> None:
        """Set environment variables for the test.

        Args:
            environ: Dictionary of environment variable names to values
        """


class MontyRunner(CodeRunner):
    """CodeRunner implementation using Monty with OSAccess virtual filesystem."""

    def __init__(self) -> None:
        self._files: list[MemoryFile] = []
        self._environ: dict[str, str] = {}
        self._os_access: OSAccess | None = None

    def write_file(self, path: str, content: str | bytes) -> None:
        # Use relative paths - OSAccess now supports them
        self._files.append(MemoryFile(path, content=content))
        # Reset OSAccess so it gets rebuilt with new files
        self._os_access = None

    def set_environ(self, environ: dict[str, str]) -> None:
        self._environ = environ
        # Reset OSAccess so it gets rebuilt with new environ
        self._os_access = None

    def _get_os_access(self) -> OSAccess:
        if self._os_access is None:
            self._os_access = OSAccess(self._files, environ=self._environ)
        return self._os_access

    def run_code(self, code: str) -> Any:
        # Prepend imports - OSAccess now handles relative paths
        wrapped_code = f'from pathlib import Path\nimport os\n{code}'
        m = Monty(wrapped_code)
        return m.run(os=self._get_os_access())

    def tree(self) -> TreeDict:
        result: TreeDict = {}

        def add_to_tree(tree: TreeDict, parts: list[str], content: str | bytes) -> None:
            if len(parts) == 1:
                tree[parts[0]] = content
            else:
                if parts[0] not in tree:
                    tree[parts[0]] = {}
                sub: Any = tree[parts[0]]
                if isinstance(sub, dict):
                    add_to_tree(sub, parts[1:], content)  # type: ignore[arg-type]

        # Build tree from all files
        for file in self._files:
            if file.deleted:
                continue
            path_parts = list(file.path.parts)
            content = file.read_content()
            add_to_tree(result, path_parts, content)

        return result


class CPythonRunner(CodeRunner):
    """CodeRunner implementation using CPython with a real temp directory."""

    def __init__(self, tmp_path: Path) -> None:
        self._root = tmp_path
        self._environ: dict[str, str] = {}

    def write_file(self, path: str, content: str | bytes) -> None:
        full_path = self._root / path
        full_path.parent.mkdir(parents=True, exist_ok=True)
        if isinstance(content, bytes):
            full_path.write_bytes(content)
        else:
            full_path.write_text(content)

    def set_environ(self, environ: dict[str, str]) -> None:
        self._environ = environ

    def run_code(self, code: str) -> Any:
        import ast
        import types

        # Map absolute paths (starting with /) to the temp directory
        # This matches OSAccess behavior which normalizes relative paths to /
        root = self._root

        def rooted_path(p: str | Path) -> Path:
            path = Path(p)
            if path.is_absolute():
                # Absolute path - strip leading / and map to root
                return root / str(path).lstrip('/')
            else:
                # Relative path - prepend / then map to root
                return root / p

        # Create a mock os module with our environ
        mock_os = types.SimpleNamespace()
        mock_os.environ = self._environ

        def getenv(key: str, default: str | None = None) -> str | None:
            return self._environ.get(key, default)

        mock_os.getenv = getenv

        namespace: dict[str, Any] = {'Path': rooted_path, 'os': mock_os}
        exec(code, namespace)

        # Find the last expression result
        tree = ast.parse(code)
        if tree.body and isinstance(tree.body[-1], ast.Expr):
            last_expr = ast.Expression(tree.body[-1].value)
            compiled = compile(last_expr, '<string>', 'eval')
            return eval(compiled, namespace)
        return None

    def tree(self) -> TreeDict:
        def build_tree(path: Path) -> TreeDict:
            result: TreeDict = {}
            for item in sorted(path.iterdir()):
                if item.is_dir():
                    subtree = build_tree(item)
                    result[item.name] = subtree
                else:
                    # Try to read as text, fall back to bytes
                    try:
                        result[item.name] = item.read_text()
                    except UnicodeDecodeError:
                        result[item.name] = item.read_bytes()
            return result

        if not self._root.exists():
            return {}
        return build_tree(self._root)


@pytest.fixture(params=['monty', 'cpython'])
def runner(request: pytest.FixtureRequest, tmp_path: Path) -> CodeRunner:
    """Fixture that provides both Monty and CPython runners for comparison testing."""
    if request.param == 'monty':
        return MontyRunner()
    else:
        return CPythonRunner(tmp_path)


# =============================================================================
# Path Existence Tests
# =============================================================================


def test_path_exists_file(runner: CodeRunner) -> None:
    """Path.exists() returns True for existing files."""
    runner.write_file('test/file.txt', 'hello')
    result = runner.run_code("Path('/test/file.txt').exists()")
    assert result is True


def test_path_exists_directory(runner: CodeRunner) -> None:
    """Path.exists() returns True for directories."""
    runner.write_file('test/subdir/file.txt', 'hello')
    result = runner.run_code("Path('/test/subdir').exists()")
    assert result is True


def test_path_exists_missing(runner: CodeRunner) -> None:
    """Path.exists() returns False for non-existent paths."""
    result = runner.run_code("Path('/missing/file.txt').exists()")
    assert result is False


def test_path_is_file(runner: CodeRunner) -> None:
    """Path.is_file() returns True for files, False for directories."""
    runner.write_file('test/file.txt', 'hello')
    assert runner.run_code("Path('/test/file.txt').is_file()") is True
    assert runner.run_code("Path('/test').is_file()") is False


def test_path_is_dir(runner: CodeRunner) -> None:
    """Path.is_dir() returns True for directories, False for files."""
    runner.write_file('test/file.txt', 'hello')
    assert runner.run_code("Path('/test').is_dir()") is True
    assert runner.run_code("Path('/test/file.txt').is_dir()") is False


# =============================================================================
# Reading Files
# =============================================================================


def test_read_text(runner: CodeRunner) -> None:
    """Path.read_text() returns file content as string."""
    runner.write_file('data/hello.txt', 'hello world')
    result = runner.run_code("Path('/data/hello.txt').read_text()")
    assert result == 'hello world'


def test_read_bytes(runner: CodeRunner) -> None:
    """Path.read_bytes() returns file content as bytes."""
    runner.write_file('data/binary.bin', b'\x00\x01\x02\x03')
    result = runner.run_code("Path('/data/binary.bin').read_bytes()")
    assert result == b'\x00\x01\x02\x03'


def test_read_text_unicode(runner: CodeRunner) -> None:
    """Path.read_text() handles unicode content."""
    runner.write_file('unicode.txt', 'hello \u2603 world')
    result = runner.run_code("Path('/unicode.txt').read_text()")
    assert result == 'hello \u2603 world'


# =============================================================================
# Tree Verification
# =============================================================================


def test_tree_simple(runner: CodeRunner) -> None:
    """tree() returns correct structure for simple files."""
    runner.write_file('a.txt', 'content a')
    runner.write_file('b.txt', 'content b')
    assert runner.tree() == {'a.txt': 'content a', 'b.txt': 'content b'}


def test_tree_nested(runner: CodeRunner) -> None:
    """tree() returns correct structure for nested directories."""
    runner.write_file('dir/subdir/file.txt', 'nested content')
    assert runner.tree() == {'dir': {'subdir': {'file.txt': 'nested content'}}}


def test_tree_mixed(runner: CodeRunner) -> None:
    """tree() handles mixed files and directories."""
    runner.write_file('root.txt', 'root')
    runner.write_file('dir/file.txt', 'in dir')
    expected = {'root.txt': 'root', 'dir': {'file.txt': 'in dir'}}
    assert runner.tree() == expected


# =============================================================================
# Stat Operations
# =============================================================================


def test_stat_size(runner: CodeRunner) -> None:
    """Path.stat().st_size returns correct file size."""
    runner.write_file('sized.txt', 'hello')
    result = runner.run_code("Path('/sized.txt').stat().st_size")
    assert result == 5


def test_stat_size_unicode(runner: CodeRunner) -> None:
    """Path.stat().st_size returns byte size for unicode content."""
    # Unicode snowman is 3 bytes in UTF-8
    runner.write_file('unicode.txt', '\u2603')
    result = runner.run_code("Path('/unicode.txt').stat().st_size")
    assert result == 3


# =============================================================================
# Directory Listing
# =============================================================================


def test_iterdir(runner: CodeRunner) -> None:
    """Path.iterdir() lists directory contents.

    Note: Monty returns filenames as strings while CPython returns Path objects
    with full paths. We normalize by getting .name (or using the string directly
    for Monty). Sorting is done in Python due to Monty limitations.
    """
    runner.write_file('dir/a.txt', 'a')
    runner.write_file('dir/b.txt', 'b')
    runner.write_file('dir/subdir/c.txt', 'c')
    # Get filenames - Monty returns strings, CPython returns Paths with full path
    # Use list() to collect, then sort in Python
    result = runner.run_code("list(Path('/dir').iterdir())")
    # Normalize: Monty gives strings, CPython gives Paths
    if isinstance(result[0], str):
        names = result  # Monty: already filenames
    else:
        names = [p.name for p in result]  # CPython: extract name from Path
    assert sorted(names) == ['a.txt', 'b.txt', 'subdir']


# =============================================================================
# Error Cases - FileNotFoundError
# =============================================================================


def test_read_text_file_not_found(runner: CodeRunner) -> None:
    """Path.read_text() raises FileNotFoundError for missing files."""
    result = runner.run_code("""
result = None
try:
    Path('/missing.txt').read_text()
except FileNotFoundError as e:
    result = type(e).__name__
result
""")
    assert result == 'FileNotFoundError'


def test_read_bytes_file_not_found(runner: CodeRunner) -> None:
    """Path.read_bytes() raises FileNotFoundError for missing files."""
    result = runner.run_code("""
result = None
try:
    Path('/missing.bin').read_bytes()
except FileNotFoundError as e:
    result = type(e).__name__
result
""")
    assert result == 'FileNotFoundError'


def test_stat_file_not_found(runner: CodeRunner) -> None:
    """Path.stat() raises FileNotFoundError for missing files."""
    result = runner.run_code("""
result = None
try:
    Path('/missing.txt').stat()
except FileNotFoundError as e:
    result = type(e).__name__
result
""")
    assert result == 'FileNotFoundError'


def test_iterdir_not_found(runner: CodeRunner) -> None:
    """Path.iterdir() raises FileNotFoundError for missing directories."""
    result = runner.run_code("""
result = None
try:
    list(Path('/missing_dir').iterdir())
except FileNotFoundError as e:
    result = type(e).__name__
result
""")
    assert result == 'FileNotFoundError'


# =============================================================================
# Error Cases - IsADirectoryError
# =============================================================================


def test_read_text_is_directory(runner: CodeRunner) -> None:
    """Path.read_text() raises IsADirectoryError when path is a directory."""
    runner.write_file('mydir/file.txt', 'content')
    result = runner.run_code("""
result = None
try:
    Path('/mydir').read_text()
except IsADirectoryError as e:
    result = type(e).__name__
result
""")
    assert result == 'IsADirectoryError'


def test_read_bytes_is_directory(runner: CodeRunner) -> None:
    """Path.read_bytes() raises IsADirectoryError when path is a directory."""
    runner.write_file('mydir/file.txt', 'content')
    result = runner.run_code("""
result = None
try:
    Path('/mydir').read_bytes()
except IsADirectoryError as e:
    result = type(e).__name__
result
""")
    assert result == 'IsADirectoryError'


# =============================================================================
# Error Cases - NotADirectoryError
# =============================================================================


def test_iterdir_not_a_directory(runner: CodeRunner) -> None:
    """Path.iterdir() raises NotADirectoryError when path is a file."""
    runner.write_file('file.txt', 'content')
    result = runner.run_code("""
result = None
try:
    list(Path('/file.txt').iterdir())
except NotADirectoryError as e:
    result = type(e).__name__
result
""")
    assert result == 'NotADirectoryError'


# =============================================================================
# Error Cases - FileExistsError
# =============================================================================


def test_mkdir_file_exists(runner: CodeRunner) -> None:
    """Path.mkdir() raises FileExistsError when directory already exists."""
    runner.write_file('existing_dir/file.txt', 'content')
    result = runner.run_code("""
result = None
try:
    Path('/existing_dir').mkdir()
except FileExistsError as e:
    result = type(e).__name__
result
""")
    assert result == 'FileExistsError'


def test_mkdir_file_at_path(runner: CodeRunner) -> None:
    """Path.mkdir() raises FileExistsError when a file exists at the path."""
    runner.write_file('somefile.txt', 'content')
    result = runner.run_code("""
result = None
try:
    Path('/somefile.txt').mkdir()
except FileExistsError as e:
    result = type(e).__name__
result
""")
    assert result == 'FileExistsError'


def test_mkdir_exist_ok_no_error(runner: CodeRunner) -> None:
    """Path.mkdir(exist_ok=True) doesn't raise when directory exists."""
    runner.write_file('existing_dir/file.txt', 'content')
    result = runner.run_code("""
Path('/existing_dir').mkdir(exist_ok=True)
'no error'
""")
    assert result == 'no error'


# =============================================================================
# Error Cases - mkdir parent not found
# =============================================================================


def test_mkdir_parent_not_found(runner: CodeRunner) -> None:
    """Path.mkdir() raises FileNotFoundError when parent doesn't exist."""
    result = runner.run_code("""
result = None
try:
    Path('/no/parent/here').mkdir()
except FileNotFoundError as e:
    result = type(e).__name__
result
""")
    assert result == 'FileNotFoundError'


def test_mkdir_parents_creates_all(runner: CodeRunner) -> None:
    """Path.mkdir(parents=True) creates all parent directories."""
    result = runner.run_code("""
Path('/a/b/c/d').mkdir(parents=True)
Path('/a/b/c/d').is_dir()
""")
    assert result is True


# =============================================================================
# Error Cases - unlink
# =============================================================================


def test_unlink_file_not_found(runner: CodeRunner) -> None:
    """Path.unlink() raises FileNotFoundError for missing files."""
    result = runner.run_code("""
result = None
try:
    Path('/missing.txt').unlink()
except FileNotFoundError as e:
    result = type(e).__name__
result
""")
    assert result == 'FileNotFoundError'


def test_unlink_is_directory(runner: CodeRunner) -> None:
    """Path.unlink() raises an error when path is a directory.

    Note: On macOS, CPython raises PermissionError for unlink() on directories,
    while Linux raises IsADirectoryError. OSAccess consistently raises IsADirectoryError.
    """
    runner.write_file('mydir/file.txt', 'content')
    # Use OSError as catch-all since PermissionError and IsADirectoryError are subclasses
    result = runner.run_code("""
result = None
try:
    Path('/mydir').unlink()
except OSError as e:
    result = type(e).__name__
result
""")
    # OSAccess raises IsADirectoryError, CPython on macOS raises PermissionError
    assert result in ('IsADirectoryError', 'PermissionError')


# =============================================================================
# Error Cases - rmdir
# =============================================================================


def test_rmdir_not_found(runner: CodeRunner) -> None:
    """Path.rmdir() raises FileNotFoundError for missing directories."""
    result = runner.run_code("""
result = None
try:
    Path('/missing_dir').rmdir()
except FileNotFoundError as e:
    result = type(e).__name__
result
""")
    assert result == 'FileNotFoundError'


def test_rmdir_not_a_directory(runner: CodeRunner) -> None:
    """Path.rmdir() raises NotADirectoryError when path is a file."""
    runner.write_file('file.txt', 'content')
    result = runner.run_code("""
result = None
try:
    Path('/file.txt').rmdir()
except NotADirectoryError as e:
    result = type(e).__name__
result
""")
    assert result == 'NotADirectoryError'


def test_rmdir_not_empty(runner: CodeRunner) -> None:
    """Path.rmdir() raises OSError when directory is not empty."""
    runner.write_file('nonempty/file.txt', 'content')
    result = runner.run_code("""
result = None
try:
    Path('/nonempty').rmdir()
except OSError as e:
    result = type(e).__name__
result
""")
    assert result == 'OSError'


# =============================================================================
# Error Cases - rename
# =============================================================================


def test_rename_source_not_found(runner: CodeRunner) -> None:
    """Path.rename() raises FileNotFoundError when source doesn't exist."""
    result = runner.run_code("""
result = None
try:
    Path('/missing.txt').rename(Path('/new.txt'))
except FileNotFoundError as e:
    result = type(e).__name__
result
""")
    assert result == 'FileNotFoundError'


# =============================================================================
# Write Operations
# =============================================================================


def test_write_text_new_file(runner: CodeRunner) -> None:
    """Path.write_text() creates a new file and returns character count."""
    result = runner.run_code("""
count = Path('/new_file.txt').write_text('hello world')
(count, Path('/new_file.txt').read_text())
""")
    assert result == (11, 'hello world')


def test_write_text_overwrite(runner: CodeRunner) -> None:
    """Path.write_text() overwrites existing files."""
    runner.write_file('existing.txt', 'old content')
    result = runner.run_code("""
Path('/existing.txt').write_text('new content')
Path('/existing.txt').read_text()
""")
    assert result == 'new content'


def test_write_bytes_new_file(runner: CodeRunner) -> None:
    """Path.write_bytes() creates a new file and returns byte count."""
    result = runner.run_code("""
count = Path('/new_binary.bin').write_bytes(b'\\x00\\x01\\x02')
(count, Path('/new_binary.bin').read_bytes())
""")
    assert result == (3, b'\x00\x01\x02')


def test_write_text_parent_not_found(runner: CodeRunner) -> None:
    """Path.write_text() raises FileNotFoundError when parent doesn't exist."""
    result = runner.run_code("""
result = None
try:
    Path('/no/parent/file.txt').write_text('content')
except FileNotFoundError as e:
    result = type(e).__name__
result
""")
    assert result == 'FileNotFoundError'


def test_write_text_to_directory(runner: CodeRunner) -> None:
    """Path.write_text() raises IsADirectoryError when writing to a directory."""
    runner.write_file('mydir/file.txt', 'content')
    result = runner.run_code("""
result = None
try:
    Path('/mydir').write_text('content')
except IsADirectoryError as e:
    result = type(e).__name__
result
""")
    assert result == 'IsADirectoryError'


# =============================================================================
# Environment Variable Tests
# =============================================================================


def test_environ_key_access(runner: CodeRunner) -> None:
    """os.environ['KEY'] returns the value for existing keys."""
    runner.set_environ({'MY_VAR': 'my_value'})
    result = runner.run_code("os.environ['MY_VAR']")
    assert result == 'my_value'


def test_environ_get_method(runner: CodeRunner) -> None:
    """os.environ.get() returns the value for existing keys."""
    runner.set_environ({'MY_VAR': 'my_value'})
    result = runner.run_code("os.environ.get('MY_VAR')")
    assert result == 'my_value'


def test_environ_get_missing_with_default(runner: CodeRunner) -> None:
    """os.environ.get() returns default for missing keys."""
    runner.set_environ({})
    result = runner.run_code("os.environ.get('MISSING', 'fallback')")
    assert result == 'fallback'


def test_environ_missing_key_raises_keyerror(runner: CodeRunner) -> None:
    """os.environ['MISSING'] raises KeyError with consistent message."""
    runner.set_environ({})
    result = runner.run_code("""
result = None
try:
    os.environ['NONEXISTENT_KEY']
except KeyError as e:
    result = str(e)
result
""")
    # Both Monty and CPython should produce the same KeyError message format
    assert result == "'NONEXISTENT_KEY'"
