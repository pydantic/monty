"""Tests for OSAccess class functionality.

These tests verify the OSAccess class behavior - the high-level virtual filesystem
that can be passed to Monty.run(os=...). Most tests run Python code through Monty
to verify behavior as it would be used in practice.

For tests of the AbstractOS interface via custom subclasses, see test_os_access_raw.py.
"""

from pathlib import PurePosixPath
from typing import Any

import pytest
from inline_snapshot import snapshot

from pydantic_monty import CallbackFile, MemoryFile, Monty, MontyRuntimeError, OSAccess

# Alias for brevity in tests
P = PurePosixPath

# =============================================================================
# OSAccess Initialization & Validation
# =============================================================================


def test_non_absolute_path():
    """OSAccess rejects files with relative paths."""
    osa = OSAccess([MemoryFile('relative/path.txt', content='test')])
    assert osa.files[0].path.as_posix() == '/relative/path.txt'

    osa = OSAccess([MemoryFile('relative/path.txt', content='test')], root_dir='/foo/bar')
    assert osa.files[0].path.as_posix() == '/foo/bar/relative/path.txt'


def test_file_nested_within_file_rejected():
    """OSAccess rejects files nested within another file's path."""
    with pytest.raises(ValueError) as exc_info:
        OSAccess(
            [
                MemoryFile('/test/file.txt', content='outer'),
                MemoryFile('/test/file.txt/nested.txt', content='inner'),
            ]
        )
    assert str(exc_info.value) == snapshot(
        "Cannot put file MemoryFile(path=/test/file.txt/nested.txt, content='...', permissions=420) "
        "within sub-directory of file MemoryFile(path=/test/file.txt, content='...', permissions=420)"
    )


def test_empty_initialization():
    """OSAccess can be initialized with no files."""
    fs = OSAccess()
    result = Monty('from pathlib import Path; Path("/any/path").exists()').run(os=fs)
    assert result is False


def test_environ_parameter():
    """OSAccess accepts environ parameter for environment variables."""
    fs = OSAccess(environ={'MY_VAR': 'my_value'})
    result = Monty("import os; os.getenv('MY_VAR')").run(os=fs)
    assert result == snapshot('my_value')


# =============================================================================
# Path Existence Checks (via Monty)
# =============================================================================


def test_path_exists_file():
    """path_exists returns True for existing files."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])
    result = Monty('from pathlib import Path; Path("/test/file.txt").exists()').run(os=fs)
    assert result is True


def test_path_exists_directory():
    """path_exists returns True for directories created by file paths."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])
    result = Monty('from pathlib import Path; Path("/test/subdir").exists()').run(os=fs)
    assert result is True


def test_path_exists_nested():
    """path_exists handles deeply nested paths."""
    fs = OSAccess([MemoryFile('/a/b/c/d/file.txt', content='deep')])
    code = """
from pathlib import Path
(Path('/a').exists(), Path('/a/b').exists(), Path('/a/b/c').exists(), Path('/a/b/c/d').exists())
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot((True, True, True, True))


def test_path_exists_missing():
    """path_exists returns False for non-existent paths."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])
    result = Monty('from pathlib import Path; Path("/other/path").exists()').run(os=fs)
    assert result is False


def test_path_is_file_for_file():
    """path_is_file returns True for files."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])
    result = Monty('from pathlib import Path; Path("/test/file.txt").is_file()').run(os=fs)
    assert result is True


def test_path_is_file_for_directory():
    """path_is_file returns False for directories."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])
    result = Monty('from pathlib import Path; Path("/test/subdir").is_file()').run(os=fs)
    assert result is False


def test_path_is_file_missing():
    """path_is_file returns False for non-existent paths."""
    fs = OSAccess()
    result = Monty('from pathlib import Path; Path("/missing").is_file()').run(os=fs)
    assert result is False


def test_path_is_dir_for_directory():
    """path_is_dir returns True for directories."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])
    result = Monty('from pathlib import Path; Path("/test/subdir").is_dir()').run(os=fs)
    assert result is True


def test_path_is_dir_for_file():
    """path_is_dir returns False for files."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])
    result = Monty('from pathlib import Path; Path("/test/file.txt").is_dir()').run(os=fs)
    assert result is False


def test_path_is_dir_missing():
    """path_is_dir returns False for non-existent paths."""
    fs = OSAccess()
    result = Monty('from pathlib import Path; Path("/missing").is_dir()').run(os=fs)
    assert result is False


def test_path_is_symlink_always_false():
    """path_is_symlink always returns False (no symlink support)."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])
    code = """
from pathlib import Path
(Path('/test/file.txt').is_symlink(), Path('/test').is_symlink(), Path('/missing').is_symlink())
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot((False, False, False))


# =============================================================================
# Reading Files (via Monty)
# =============================================================================


def test_read_text_string_content():
    """path_read_text returns string content directly."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello world')])
    result = Monty('from pathlib import Path; Path("/test/file.txt").read_text()').run(os=fs)
    assert result == snapshot('hello world')


def test_read_text_bytes_content_decoded():
    """path_read_text decodes bytes content as UTF-8."""
    fs = OSAccess([MemoryFile('/test/file.txt', content=b'bytes content')])
    result = Monty('from pathlib import Path; Path("/test/file.txt").read_text()').run(os=fs)
    assert result == snapshot('bytes content')


def test_read_bytes_bytes_content():
    """path_read_bytes returns bytes content directly."""
    fs = OSAccess([MemoryFile('/test/file.bin', content=b'\x00\x01\x02\x03')])
    result = Monty('from pathlib import Path; Path("/test/file.bin").read_bytes()').run(os=fs)
    assert result == snapshot(b'\x00\x01\x02\x03')


def test_read_bytes_string_content_encoded():
    """path_read_bytes encodes string content as UTF-8."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])
    result = Monty('from pathlib import Path; Path("/test/file.txt").read_bytes()').run(os=fs)
    assert result == snapshot(b'hello')


def test_read_text_file_not_found():
    """path_read_text raises FileNotFoundError for missing files."""
    fs = OSAccess()
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty('from pathlib import Path; Path("/missing.txt").read_text()').run(os=fs)
    assert str(exc_info.value) == snapshot("FileNotFoundError: [Errno 2] No such file or directory: '/missing.txt'")


def test_read_bytes_file_not_found():
    """path_read_bytes raises FileNotFoundError for missing files."""
    fs = OSAccess()
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty('from pathlib import Path; Path("/missing.bin").read_bytes()').run(os=fs)
    assert str(exc_info.value) == snapshot("FileNotFoundError: [Errno 2] No such file or directory: '/missing.bin'")


def test_read_text_is_a_directory():
    """path_read_text raises error for directories."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty('from pathlib import Path; Path("/test/subdir").read_text()').run(os=fs)
    # Monty reports this as OSError, not IsADirectoryError
    assert str(exc_info.value) == snapshot("IsADirectoryError: [Errno 21] Is a directory: '/test/subdir'")


def test_read_bytes_is_a_directory():
    """path_read_bytes raises error for directories."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty('from pathlib import Path; Path("/test/subdir").read_bytes()').run(os=fs)
    # Monty reports this as OSError, not IsADirectoryError
    assert str(exc_info.value) == snapshot("IsADirectoryError: [Errno 21] Is a directory: '/test/subdir'")


# =============================================================================
# Writing Files (via Monty)
# =============================================================================


def test_write_text_via_monty():
    """Path.write_text() creates a new file via Monty."""
    fs = OSAccess([MemoryFile('/test/existing.txt', content='existing')])

    code = """
from pathlib import Path
Path('/test/new.txt').write_text('new content')
"""
    result = Monty(code).run(os=fs)
    # write_text returns the number of bytes written
    assert result == snapshot(11)

    # Verify file was created
    assert fs.path_exists(P('/test/new.txt')) is True
    assert fs.path_read_text(P('/test/new.txt')) == 'new content'


def test_write_text_overwrite_via_monty():
    """Path.write_text() overwrites existing file via Monty."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='original')])

    code = """
from pathlib import Path
Path('/test/file.txt').write_text('updated')
"""
    Monty(code).run(os=fs)
    assert fs.path_read_text(P('/test/file.txt')) == 'updated'


def test_write_bytes_via_monty():
    """Path.write_bytes() creates a new file via Monty."""
    fs = OSAccess([MemoryFile('/test/existing.txt', content='existing')])

    code = """
from pathlib import Path
Path('/test/new.bin').write_bytes(b'binary data')
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot(11)
    assert fs.path_read_bytes(P('/test/new.bin')) == b'binary data'


def test_write_text_parent_not_exists_via_monty():
    """Path.write_text() raises FileNotFoundError when parent doesn't exist via Monty."""
    fs = OSAccess()
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/no/parent/file.txt').write_text('test')").run(os=fs)
    assert str(exc_info.value) == snapshot(
        "FileNotFoundError: [Errno 2] No such file or directory: '/no/parent/file.txt'"
    )


def test_write_text_to_directory_via_monty():
    """Path.write_text() raises IsADirectoryError when writing to a directory via Monty."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/test/subdir').write_text('test')").run(os=fs)
    assert str(exc_info.value) == snapshot("IsADirectoryError: [Errno 21] Is a directory: '/test/subdir'")


# =============================================================================
# Writing Files (via direct API)
# =============================================================================


def test_write_text_new_file_direct():
    """path_write_text creates a new file via direct API."""
    fs = OSAccess([MemoryFile('/test/existing.txt', content='existing')])

    # Write a new file
    fs.path_write_text(P('/test/new.txt'), 'new content')

    # Verify it was created
    assert fs.path_exists(P('/test/new.txt')) is True
    assert fs.path_read_text(P('/test/new.txt')) == 'new content'


def test_write_text_overwrite_existing_direct():
    """path_write_text overwrites existing file content via direct API."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='original')])

    fs.path_write_text(P('/test/file.txt'), 'updated')
    assert fs.path_read_text(P('/test/file.txt')) == 'updated'


def test_write_bytes_new_file_direct():
    """path_write_bytes creates a new file via direct API."""
    fs = OSAccess([MemoryFile('/test/existing.txt', content='existing')])

    fs.path_write_bytes(P('/test/new.bin'), b'binary data')
    assert fs.path_read_bytes(P('/test/new.bin')) == b'binary data'


def test_write_bytes_overwrite_existing_direct():
    """path_write_bytes overwrites existing file content via direct API."""
    fs = OSAccess([MemoryFile('/test/file.bin', content=b'original')])

    fs.path_write_bytes(P('/test/file.bin'), b'updated')
    assert fs.path_read_bytes(P('/test/file.bin')) == b'updated'


def test_write_text_parent_not_exists_direct():
    """path_write_text raises FileNotFoundError when parent doesn't exist via direct API."""
    fs = OSAccess()
    with pytest.raises(FileNotFoundError) as exc_info:
        fs.path_write_text(P('/no/parent/file.txt'), 'test')
    assert str(exc_info.value) == snapshot("[Errno 2] No such file or directory: '/no/parent/file.txt'")


def test_write_text_to_directory_direct():
    """path_write_text raises IsADirectoryError when writing to a directory via direct API."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])
    with pytest.raises(IsADirectoryError) as exc_info:
        fs.path_write_text(P('/test/subdir'), 'test')
    assert str(exc_info.value) == snapshot("[Errno 21] Is a directory: '/test/subdir'")


# =============================================================================
# Directory Operations - mkdir (via Monty)
# =============================================================================


def test_mkdir_basic_via_monty():
    """Path.mkdir() creates a directory via Monty."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])

    code = """
from pathlib import Path
Path('/test/newdir').mkdir()
"""
    Monty(code).run(os=fs)
    assert fs.path_is_dir(P('/test/newdir')) is True


def test_mkdir_with_parents_via_monty():
    """Path.mkdir(parents=True) creates parent directories via Monty."""
    fs = OSAccess()

    code = """
from pathlib import Path
Path('/a/b/c/d').mkdir(parents=True)
"""
    Monty(code).run(os=fs)
    assert fs.path_is_dir(P('/a')) is True
    assert fs.path_is_dir(P('/a/b')) is True
    assert fs.path_is_dir(P('/a/b/c')) is True
    assert fs.path_is_dir(P('/a/b/c/d')) is True


def test_mkdir_exist_ok_true_via_monty():
    """Path.mkdir(exist_ok=True) doesn't raise for existing directory via Monty."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])

    code = """
from pathlib import Path
Path('/test/subdir').mkdir(exist_ok=True)
"""
    # Should not raise
    Monty(code).run(os=fs)
    assert fs.path_is_dir(P('/test/subdir')) is True


def test_mkdir_exist_ok_false_via_monty():
    """Path.mkdir() raises OSError (FileExistsError) for existing directory via Monty."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])

    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/test/subdir').mkdir()").run(os=fs)
    # Monty maps FileExistsError to OSError
    assert str(exc_info.value) == snapshot("FileExistsError: [Errno 17] File exists: '/test/subdir'")


def test_mkdir_parent_not_exists_via_monty():
    """Path.mkdir() raises FileNotFoundError when parent doesn't exist via Monty."""
    fs = OSAccess()

    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/no/parent/dir').mkdir()").run(os=fs)
    assert str(exc_info.value) == snapshot("FileNotFoundError: [Errno 2] No such file or directory: '/no/parent/dir'")


# =============================================================================
# Directory Operations - mkdir (via direct API)
# =============================================================================


def test_mkdir_basic_direct():
    """path_mkdir creates a directory via direct API."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])

    fs.path_mkdir(P('/test/newdir'), parents=False, exist_ok=False)
    assert fs.path_is_dir(P('/test/newdir')) is True


def test_mkdir_with_parents_direct():
    """path_mkdir with parents=True creates parent directories via direct API."""
    fs = OSAccess()

    fs.path_mkdir(P('/a/b/c/d'), parents=True, exist_ok=False)
    assert fs.path_is_dir(P('/a')) is True
    assert fs.path_is_dir(P('/a/b')) is True
    assert fs.path_is_dir(P('/a/b/c')) is True
    assert fs.path_is_dir(P('/a/b/c/d')) is True


def test_mkdir_exist_ok_true_direct():
    """path_mkdir with exist_ok=True doesn't raise for existing directory via direct API."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])

    # Should not raise
    fs.path_mkdir(P('/test/subdir'), parents=False, exist_ok=True)
    assert fs.path_is_dir(P('/test/subdir')) is True


def test_mkdir_exist_ok_false_direct():
    """path_mkdir with exist_ok=False raises for existing directory via direct API."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])

    with pytest.raises(FileExistsError) as exc_info:
        fs.path_mkdir(P('/test/subdir'), parents=False, exist_ok=False)
    assert str(exc_info.value) == snapshot("[Errno 17] File exists: '/test/subdir'")


def test_mkdir_file_exists_direct():
    """path_mkdir raises FileExistsError when a file exists at the path via direct API."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])

    with pytest.raises(FileExistsError) as exc_info:
        fs.path_mkdir(P('/test/file.txt'), parents=False, exist_ok=False)
    assert str(exc_info.value) == snapshot("[Errno 17] File exists: '/test/file.txt'")


def test_mkdir_parent_not_exists_direct():
    """path_mkdir without parents raises FileNotFoundError when parent doesn't exist via direct API."""
    fs = OSAccess()

    with pytest.raises(FileNotFoundError) as exc_info:
        fs.path_mkdir(P('/no/parent/dir'), parents=False, exist_ok=False)
    assert str(exc_info.value) == snapshot("[Errno 2] No such file or directory: '/no/parent/dir'")


def test_mkdir_parent_is_file_direct():
    """path_mkdir raises NotADirectoryError when parent is a file via direct API."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])

    with pytest.raises(NotADirectoryError) as exc_info:
        fs.path_mkdir(P('/test/file.txt/subdir'), parents=True, exist_ok=False)
    assert str(exc_info.value) == snapshot("[Errno 20] Not a directory: '/test/file.txt/subdir'")


# =============================================================================
# Directory Operations - rmdir (via Monty)
# =============================================================================


def test_rmdir_empty_directory_via_monty():
    """Path.rmdir() removes an empty directory via Monty."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])
    fs.path_mkdir(P('/test/newdir'), parents=False, exist_ok=False)

    code = """
from pathlib import Path
Path('/test/newdir').rmdir()
"""
    Monty(code).run(os=fs)
    assert fs.path_exists(P('/test/newdir')) is False


def test_rmdir_non_empty_directory_via_monty():
    """Path.rmdir() raises OSError for non-empty directory via Monty."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])

    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/test/subdir').rmdir()").run(os=fs)
    assert str(exc_info.value) == snapshot("OSError: [Errno 39] Directory not empty: '/test/subdir'")


def test_rmdir_not_found_via_monty():
    """Path.rmdir() raises FileNotFoundError for non-existent path via Monty."""
    fs = OSAccess()

    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/missing').rmdir()").run(os=fs)
    assert str(exc_info.value) == snapshot("FileNotFoundError: [Errno 2] No such file or directory: '/missing'")


def test_rmdir_file_not_directory_via_monty():
    """Path.rmdir() raises NotADirectoryError for files via Monty."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])

    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/test/file.txt').rmdir()").run(os=fs)
    assert str(exc_info.value) == snapshot("NotADirectoryError: [Errno 20] Not a directory: '/test/file.txt'")


# =============================================================================
# Directory Operations - rmdir (via direct API)
# =============================================================================


def test_rmdir_empty_directory_direct():
    """path_rmdir removes an empty directory via direct API."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])

    fs.path_mkdir(P('/test/newdir'), parents=False, exist_ok=False)
    fs.path_rmdir(P('/test/newdir'))
    assert fs.path_exists(P('/test/newdir')) is False


def test_rmdir_non_empty_directory_direct():
    """path_rmdir raises OSError for non-empty directory via direct API."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])

    with pytest.raises(OSError) as exc_info:
        fs.path_rmdir(P('/test/subdir'))
    assert str(exc_info.value) == snapshot("[Errno 39] Directory not empty: '/test/subdir'")


def test_rmdir_file_not_directory_direct():
    """path_rmdir raises NotADirectoryError for files via direct API."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])

    with pytest.raises(NotADirectoryError) as exc_info:
        fs.path_rmdir(P('/test/file.txt'))
    assert str(exc_info.value) == snapshot("[Errno 20] Not a directory: '/test/file.txt'")


def test_rmdir_not_found_direct():
    """path_rmdir raises FileNotFoundError for non-existent path via direct API."""
    fs = OSAccess()

    with pytest.raises(FileNotFoundError) as exc_info:
        fs.path_rmdir(P('/missing'))
    assert str(exc_info.value) == snapshot("[Errno 2] No such file or directory: '/missing'")


# =============================================================================
# Directory Operations - iterdir (via Monty)
# =============================================================================


def test_iterdir_list_contents():
    """path_iterdir lists directory contents."""
    fs = OSAccess(
        [
            MemoryFile('/test/a.txt', content='a'),
            MemoryFile('/test/b.txt', content='b'),
            MemoryFile('/test/subdir/c.txt', content='c'),
        ]
    )
    code = """
from pathlib import Path
[str(p) for p in Path('/test').iterdir()]
"""
    result = Monty(code).run(os=fs)
    # Result may be in any order, so sort in Python
    assert sorted(result) == snapshot(['/test/a.txt', '/test/b.txt', '/test/subdir'])


def test_iterdir_empty_directory_direct():
    """path_iterdir returns empty list for empty directory via direct API."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])
    fs.path_mkdir(P('/test/empty'), parents=False, exist_ok=False)

    result = fs.path_iterdir(P('/test/empty'))
    assert result == snapshot([])


def test_iterdir_not_a_directory_direct():
    """path_iterdir raises NotADirectoryError for files via direct API."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])

    with pytest.raises(NotADirectoryError) as exc_info:
        fs.path_iterdir(P('/test/file.txt'))
    assert str(exc_info.value) == snapshot("[Errno 20] Not a directory: '/test/file.txt'")


def test_iterdir_not_found():
    """path_iterdir raises FileNotFoundError for non-existent path."""
    fs = OSAccess()
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; list(Path('/missing').iterdir())").run(os=fs)
    assert str(exc_info.value) == snapshot("FileNotFoundError: [Errno 2] No such file or directory: '/missing'")


# =============================================================================
# File Operations - unlink (via Monty)
# =============================================================================


def test_unlink_file_via_monty():
    """Path.unlink() removes a file via Monty."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])

    code = """
from pathlib import Path
Path('/test/file.txt').unlink()
"""
    Monty(code).run(os=fs)
    assert fs.path_exists(P('/test/file.txt')) is False


def test_unlink_file_not_found_via_monty():
    """Path.unlink() raises FileNotFoundError for non-existent files via Monty."""
    fs = OSAccess()

    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/missing.txt').unlink()").run(os=fs)
    assert str(exc_info.value) == snapshot("FileNotFoundError: [Errno 2] No such file or directory: '/missing.txt'")


def test_unlink_is_directory_via_monty():
    """Path.unlink() raises IsADirectoryError for directories via Monty."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])

    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/test/subdir').unlink()").run(os=fs)
    assert str(exc_info.value) == snapshot("IsADirectoryError: [Errno 21] Is a directory: '/test/subdir'")


# =============================================================================
# File Operations - unlink (via direct API)
# =============================================================================


def test_unlink_file_direct():
    """path_unlink removes a file via direct API."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])

    fs.path_unlink(P('/test/file.txt'))
    assert fs.path_exists(P('/test/file.txt')) is False


def test_unlink_file_not_found_direct():
    """path_unlink raises FileNotFoundError for non-existent files via direct API."""
    fs = OSAccess()

    with pytest.raises(FileNotFoundError) as exc_info:
        fs.path_unlink(P('/missing.txt'))
    assert str(exc_info.value) == snapshot("[Errno 2] No such file or directory: '/missing.txt'")


def test_unlink_is_directory_direct():
    """path_unlink raises IsADirectoryError for directories via direct API."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])

    with pytest.raises(IsADirectoryError) as exc_info:
        fs.path_unlink(P('/test/subdir'))
    assert str(exc_info.value) == snapshot("[Errno 21] Is a directory: '/test/subdir'")


# =============================================================================
# Stat Operations (via Monty)
# =============================================================================


def test_stat_file():
    """path_stat returns stat result for files with size and mode."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello world')])
    code = """
from pathlib import Path
s = Path('/test/file.txt').stat()
(s.st_size, s.st_mode & 0o777)
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot((11, 0o644))


def test_stat_file_custom_permissions():
    """path_stat returns custom file permissions."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello', permissions=0o755)])
    code = """
from pathlib import Path
s = Path('/test/file.txt').stat()
s.st_mode & 0o777
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot(0o755)


def test_stat_directory():
    """path_stat returns stat result for directories."""
    fs = OSAccess([MemoryFile('/test/subdir/file.txt', content='hello')])
    code = """
from pathlib import Path
s = Path('/test/subdir').stat()
s.st_mode
"""
    result = Monty(code).run(os=fs)
    # Directory mode bits: 0o040000 (directory) | 0o755 (default perms) = 0o040755
    assert result == snapshot(0o040755)


def test_stat_file_not_found():
    """path_stat raises FileNotFoundError for non-existent paths."""
    fs = OSAccess()
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/missing').stat()").run(os=fs)
    assert str(exc_info.value) == snapshot("FileNotFoundError: [Errno 2] No such file or directory: '/missing'")


def test_stat_bytes_content_size():
    """path_stat calculates size correctly for bytes content."""
    fs = OSAccess([MemoryFile('/test/file.bin', content=b'\x00\x01\x02\x03\x04')])
    code = """
from pathlib import Path
Path('/test/file.bin').stat().st_size
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot(5)


def test_stat_unicode_size():
    """path_stat calculates size as encoded UTF-8 bytes for string content."""
    # Unicode snowman is 3 bytes in UTF-8
    fs = OSAccess([MemoryFile('/test/file.txt', content='☃')])
    code = """
from pathlib import Path
Path('/test/file.txt').stat().st_size
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot(3)


# =============================================================================
# Rename Operations (via Monty)
# =============================================================================


def test_rename_file_via_monty():
    """Path.rename() renames a file via Monty."""
    fs = OSAccess([MemoryFile('/test/old.txt', content='content')])

    code = """
from pathlib import Path
Path('/test/old.txt').rename(Path('/test/new.txt'))
"""
    Monty(code).run(os=fs)

    assert fs.path_exists(P('/test/old.txt')) is False
    assert fs.path_exists(P('/test/new.txt')) is True
    assert fs.path_read_text(P('/test/new.txt')) == 'content'


def test_rename_source_not_found_via_monty():
    """Path.rename() raises FileNotFoundError when source doesn't exist via Monty."""
    fs = OSAccess()

    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/missing.txt').rename(Path('/new.txt'))").run(os=fs)
    assert str(exc_info.value) == snapshot(
        "FileNotFoundError: [Errno 2] No such file or directory: '/missing.txt' -> '/new.txt'"
    )


def test_rename_target_parent_not_found_via_monty():
    """Path.rename() raises FileNotFoundError when target parent doesn't exist via Monty."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='content')])

    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("from pathlib import Path; Path('/test/file.txt').rename(Path('/no/parent/file.txt'))").run(os=fs)
    assert str(exc_info.value) == snapshot(
        "FileNotFoundError: [Errno 2] No such file or directory: '/test/file.txt' -> '/no/parent/file.txt'"
    )


# =============================================================================
# Rename Operations (via direct API)
# =============================================================================


def test_rename_file_direct():
    """path_rename renames a file via direct API."""
    fs = OSAccess([MemoryFile('/test/old.txt', content='content')])

    fs.path_rename(P('/test/old.txt'), P('/test/new.txt'))

    assert fs.path_exists(P('/test/old.txt')) is False
    assert fs.path_exists(P('/test/new.txt')) is True
    assert fs.path_read_text(P('/test/new.txt')) == 'content'


def test_rename_source_not_found_direct():
    """path_rename raises FileNotFoundError when source doesn't exist via direct API."""
    fs = OSAccess()

    with pytest.raises(FileNotFoundError) as exc_info:
        fs.path_rename(P('/missing.txt'), P('/new.txt'))
    assert str(exc_info.value) == snapshot("[Errno 2] No such file or directory: '/missing.txt' -> '/new.txt'")


def test_rename_target_parent_not_found_direct():
    """path_rename raises FileNotFoundError when target parent doesn't exist via direct API."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='content')])

    with pytest.raises(FileNotFoundError) as exc_info:
        fs.path_rename(P('/test/file.txt'), P('/no/parent/file.txt'))
    assert str(exc_info.value) == snapshot(
        "[Errno 2] No such file or directory: '/test/file.txt' -> '/no/parent/file.txt'"
    )


def test_rename_directory_direct():
    """path_rename renames a directory via direct API."""
    fs = OSAccess([MemoryFile('/test/olddir/file.txt', content='content')])
    fs.path_mkdir(P('/test/newdir'), parents=False, exist_ok=False)

    fs.path_rename(P('/test/newdir'), P('/test/renamed'))
    assert fs.path_is_dir(P('/test/renamed')) is True


def test_rename_directory_non_empty_target_direct():
    """path_rename raises OSError when renaming directory to non-empty target via direct API."""
    fs = OSAccess(
        [
            MemoryFile('/test/src/a.txt', content='a'),
            MemoryFile('/test/dst/b.txt', content='b'),
        ]
    )

    with pytest.raises(OSError) as exc_info:
        fs.path_rename(P('/test/src'), P('/test/dst'))
    assert str(exc_info.value) == snapshot("[Errno 66] Directory not empty: '/test/src' -> '/test/dst'")


def test_rename_directory_updates_file_paths_direct():
    """path_rename updates paths of all files within renamed directory."""
    file1 = MemoryFile('/old/dir/file1.txt', content='one')
    file2 = MemoryFile('/old/dir/subdir/file2.txt', content='two')
    fs = OSAccess([file1, file2])

    # Create target parent and rename the directory
    fs.path_mkdir(P('/new'), parents=False, exist_ok=False)
    fs.path_rename(P('/old/dir'), P('/new/location'))

    # Verify files are accessible at new paths
    assert fs.path_read_text(P('/new/location/file1.txt')) == 'one'
    assert fs.path_read_text(P('/new/location/subdir/file2.txt')) == 'two'

    # Verify the AbstractFile objects have updated paths
    assert file1.path.as_posix() == '/new/location/file1.txt'
    assert file2.path.as_posix() == '/new/location/subdir/file2.txt'

    # Verify old paths no longer exist
    assert fs.path_exists(P('/old/dir')) is False
    assert fs.path_exists(P('/old/dir/file1.txt')) is False


# =============================================================================
# Path Resolution (via Monty)
# =============================================================================


def test_path_resolve_absolute():
    """path_resolve returns absolute path."""
    fs = OSAccess([MemoryFile('/test/file.txt', content='hello')])
    code = """
from pathlib import Path
str(Path('/test/file.txt').resolve())
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot('/test/file.txt')


def test_path_absolute_already_absolute():
    """path_absolute returns same path for already absolute path."""
    fs = OSAccess()
    code = """
from pathlib import Path
str(Path('/already/absolute').absolute())
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot('/already/absolute')


def test_path_absolute_relative():
    """path_absolute converts relative path to absolute."""
    fs = OSAccess()
    code = """
from pathlib import Path
str(Path('relative/path').absolute())
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot('/relative/path')


def test_path_resolve_same_as_absolute():
    """path_resolve behaves same as absolute (no symlinks in OSAccess)."""
    fs = OSAccess()
    code = """
from pathlib import Path
str(Path('relative').resolve()) == str(Path('relative').absolute())
"""
    result = Monty(code).run(os=fs)
    assert result is True


# =============================================================================
# Environment Variables (via Monty)
# =============================================================================


def test_getenv_existing_key():
    """getenv returns value for existing key."""
    fs = OSAccess(environ={'MY_VAR': 'my_value'})
    result = Monty("import os; os.getenv('MY_VAR')").run(os=fs)
    assert result == snapshot('my_value')


def test_getenv_missing_key():
    """getenv returns None for missing key."""
    fs = OSAccess(environ={'OTHER': 'value'})
    result = Monty("import os; os.getenv('MISSING')").run(os=fs)
    assert result is None


def test_getenv_missing_with_default():
    """getenv returns default for missing key when default provided."""
    fs = OSAccess(environ={})
    result = Monty("import os; os.getenv('MISSING', 'default_value')").run(os=fs)
    assert result == snapshot('default_value')


def test_getenv_multiple_vars():
    """getenv handles multiple environment variables."""
    fs = OSAccess(environ={'VAR1': 'value1', 'VAR2': 'value2', 'VAR3': 'value3'})
    code = """
import os
(os.getenv('VAR1'), os.getenv('VAR2'), os.getenv('VAR3'))
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot(('value1', 'value2', 'value3'))


def test_get_environ_returns_dict():
    """os.environ returns the full environ dict."""
    fs = OSAccess(environ={'HOME': '/home/user', 'USER': 'testuser'})
    result = Monty('import os; os.environ').run(os=fs)
    assert result == snapshot({'HOME': '/home/user', 'USER': 'testuser'})


def test_get_environ_key_access():
    """os.environ['KEY'] returns the value."""
    fs = OSAccess(environ={'MY_VAR': 'my_value'})
    result = Monty("import os; os.environ['MY_VAR']").run(os=fs)
    assert result == snapshot('my_value')


def test_get_environ_key_missing_raises():
    """os.environ['MISSING'] raises KeyError."""
    fs = OSAccess(environ={})
    with pytest.raises(MontyRuntimeError) as exc_info:
        Monty("import os; os.environ['MISSING']").run(os=fs)
    assert str(exc_info.value) == snapshot('KeyError: MISSING')


def test_get_environ_get_method():
    """os.environ.get() works correctly."""
    fs = OSAccess(environ={'HOME': '/home/user'})
    result = Monty("import os; os.environ.get('HOME')").run(os=fs)
    assert result == snapshot('/home/user')


def test_get_environ_get_missing_with_default():
    """os.environ.get() returns default for missing key."""
    fs = OSAccess(environ={})
    result = Monty("import os; os.environ.get('MISSING', 'fallback')").run(os=fs)
    assert result == snapshot('fallback')


def test_get_environ_len():
    """len(os.environ) returns the number of env vars."""
    fs = OSAccess(environ={'A': '1', 'B': '2', 'C': '3'})
    result = Monty('import os; len(os.environ)').run(os=fs)
    assert result == snapshot(3)


def test_get_environ_contains():
    """'KEY' in os.environ tests membership."""
    fs = OSAccess(environ={'PRESENT': 'value'})
    code = """
import os
('PRESENT' in os.environ, 'ABSENT' in os.environ)
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot((True, False))


def test_get_environ_keys():
    """os.environ.keys() returns the keys."""
    fs = OSAccess(environ={'X': '1', 'Y': '2'})
    result = Monty('import os; list(os.environ.keys())').run(os=fs)
    assert set(result) == snapshot({'X', 'Y'})


def test_get_environ_values():
    """os.environ.values() returns the values."""
    fs = OSAccess(environ={'X': 'a', 'Y': 'b'})
    result = Monty('import os; list(os.environ.values())').run(os=fs)
    assert set(result) == snapshot({'a', 'b'})


def test_get_environ_items():
    """os.environ.items() returns key-value pairs."""
    fs = OSAccess(environ={'X': '1', 'Y': '2'})
    result = Monty('import os; list(os.environ.items())').run(os=fs)
    assert set(result) == snapshot({('X', '1'), ('Y', '2')})


def test_get_environ_empty():
    """os.environ returns empty dict when no environ provided."""
    fs = OSAccess()
    result = Monty('import os; os.environ').run(os=fs)
    assert result == snapshot({})


# =============================================================================
# MemoryFile Behavior
# =============================================================================


def test_memory_file_string_content():
    """MemoryFile stores and returns string content."""
    file = MemoryFile('/test/file.txt', content='hello')
    assert file.read_content() == snapshot('hello')
    assert file.path.as_posix() == snapshot('/test/file.txt')
    assert file.name == snapshot('file.txt')


def test_memory_file_bytes_content():
    """MemoryFile stores and returns bytes content."""
    file = MemoryFile('/test/file.bin', content=b'\x00\x01\x02')
    assert file.read_content() == snapshot(b'\x00\x01\x02')


def test_memory_file_custom_permissions():
    """MemoryFile accepts custom permissions."""
    file = MemoryFile('/test/exec.sh', content='#!/bin/bash', permissions=0o755)
    assert file.permissions == snapshot(0o755)


def test_memory_file_write_and_read():
    """MemoryFile supports writing and re-reading content."""
    file = MemoryFile('/test/file.txt', content='original')
    file.write_content('updated')
    assert file.read_content() == snapshot('updated')


def test_memory_file_delete():
    """MemoryFile can be marked as deleted."""
    file = MemoryFile('/test/file.txt', content='content')
    assert file.deleted is False
    file.delete()
    assert file.deleted is True


def test_memory_file_repr():
    """MemoryFile has useful repr for debugging."""
    file = MemoryFile('/test/file.txt', content='content')
    assert repr(file) == snapshot("MemoryFile(path=/test/file.txt, content='...', permissions=420)")


def test_memory_file_bytes_repr():
    """MemoryFile repr shows b'...' for bytes content."""
    file = MemoryFile('/test/file.bin', content=b'\x00')
    assert repr(file) == snapshot("MemoryFile(path=/test/file.bin, content=b'...', permissions=420)")


# =============================================================================
# CallbackFile Behavior
# =============================================================================


def test_callback_file_read():
    """CallbackFile calls read callback."""
    read_calls: list[PurePosixPath] = []

    def read_fn(path: PurePosixPath) -> str:
        read_calls.append(path)
        return f'content from {path}'

    def write_fn(path: PurePosixPath, content: str | bytes) -> None:
        pass

    file = CallbackFile('/test/file.txt', read=read_fn, write=write_fn)
    fs = OSAccess([file])

    result = Monty('from pathlib import Path; Path("/test/file.txt").read_text()').run(os=fs)
    assert result == snapshot('content from /test/file.txt')
    assert len(read_calls) == 1


def test_callback_file_write_direct():
    """CallbackFile calls write callback via direct API."""
    written: list[tuple[PurePosixPath, Any]] = []

    def read_fn(path: PurePosixPath) -> str:
        return ''

    def write_fn(path: PurePosixPath, content: str | bytes) -> None:
        written.append((path, content))

    file = CallbackFile('/test/file.txt', read=read_fn, write=write_fn)
    fs = OSAccess([file])

    # Use direct API since write_text not implemented in Monty
    fs.path_write_text(P('/test/file.txt'), 'new content')
    assert len(written) == 1
    assert written[0][1] == snapshot('new content')


def test_callback_file_custom_permissions():
    """CallbackFile accepts custom permissions."""
    file = CallbackFile(
        '/test/file.txt',
        read=lambda _: '',
        write=lambda _p, _c: None,
        permissions=0o700,
    )
    assert file.permissions == snapshot(0o700)


def test_callback_file_repr():
    """CallbackFile has useful repr for debugging."""
    file = CallbackFile('/test/file.txt', read=lambda _: '', write=lambda _, __: None)
    assert 'CallbackFile(path=/test/file.txt' in repr(file)


# =============================================================================
# Custom AbstractFile Implementation
# =============================================================================


class CustomFile:
    """Minimal custom AbstractFile implementation."""

    def __init__(self, path: str, content: str) -> None:
        self.path = PurePosixPath(path)
        self.name = self.path.name
        self.permissions = 0o644
        self.deleted = False
        self.content = content

    def read_content(self) -> str:
        return self.content

    def write_content(self, content: str | bytes) -> None:
        self.content = content if isinstance(content, str) else content.decode()

    def delete(self) -> None:
        self.deleted = True


def test_custom_abstract_file():
    """Custom AbstractFile implementation works with OSAccess."""
    custom = CustomFile('/test/custom.txt', 'custom content')
    fs = OSAccess([custom])

    result = Monty('from pathlib import Path; Path("/test/custom.txt").read_text()').run(os=fs)
    assert result == snapshot('custom content')


def test_custom_abstract_file_mixed_with_memory_file():
    """Custom AbstractFile can be mixed with MemoryFile."""
    custom = CustomFile('/test/custom.txt', 'from custom')
    memory = MemoryFile('/test/memory.txt', content='from memory')
    fs = OSAccess([custom, memory])

    code = """
from pathlib import Path
(Path('/test/custom.txt').read_text(), Path('/test/memory.txt').read_text())
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot(('from custom', 'from memory'))


# =============================================================================
# Direct API Test (without Monty)
# =============================================================================


def test_os_access_direct_api():
    """OSAccess methods can be called directly without Monty."""
    fs = OSAccess(
        [
            MemoryFile('/test/file.txt', content='hello'),
            MemoryFile('/test/subdir/nested.txt', content='nested'),
        ]
    )

    # Test path_exists
    assert fs.path_exists(P('/test/file.txt')) is True
    assert fs.path_exists(P('/missing')) is False

    # Test path_is_file / path_is_dir
    assert fs.path_is_file(P('/test/file.txt')) is True
    assert fs.path_is_dir(P('/test/file.txt')) is False
    assert fs.path_is_dir(P('/test/subdir')) is True
    assert fs.path_is_file(P('/test/subdir')) is False

    # Test path_read_text / path_read_bytes
    assert fs.path_read_text(P('/test/file.txt')) == 'hello'
    assert fs.path_read_bytes(P('/test/file.txt')) == b'hello'

    # Test path_stat
    stat = fs.path_stat(P('/test/file.txt'))
    assert stat.st_size == 5

    # Test path_iterdir
    contents = fs.path_iterdir(P('/test'))
    assert sorted(contents) == snapshot([PurePosixPath('/test/file.txt'), PurePosixPath('/test/subdir')])

    # Test path_absolute
    assert fs.path_absolute(P('relative')) == '/relative'
    assert fs.path_absolute(P('/absolute')) == '/absolute'


# =============================================================================
# Edge Cases
# =============================================================================


def test_root_directory():
    """Root directory '/' is handled correctly."""
    fs = OSAccess([MemoryFile('/file.txt', content='root file')])
    code = """
from pathlib import Path
(Path('/').is_dir(), sorted([str(p) for p in Path('/').iterdir()]))
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot((True, ['/file.txt']))


def test_empty_file():
    """Empty file content is handled correctly."""
    fs = OSAccess([MemoryFile('/empty.txt', content='')])
    code = """
from pathlib import Path
(Path('/empty.txt').read_text(), Path('/empty.txt').stat().st_size)
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot(('', 0))


def test_large_nested_path():
    """Deeply nested paths are handled correctly."""
    fs = OSAccess([MemoryFile('/a/b/c/d/e/f/g/h/i/j/file.txt', content='deep')])
    code = """
from pathlib import Path
Path('/a/b/c/d/e/f/g/h/i/j/file.txt').read_text()
"""
    result = Monty(code).run(os=fs)
    assert result == snapshot('deep')


def test_special_characters_in_content():
    """Special characters in file content are handled correctly."""
    content = 'line1\nline2\ttab\r\nwindows'
    fs = OSAccess([MemoryFile('/special.txt', content=content)])
    result = Monty('from pathlib import Path; Path("/special.txt").read_text()').run(os=fs)
    assert result == snapshot('line1\nline2\ttab\r\nwindows')
