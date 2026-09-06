# mount-fs
import os
import sys
from pathlib import Path

is_windows = sys.platform == 'win32'

# root is injected by the test runner:
# - Monty: Path('/mnt'), also the sandbox working directory
# - CPython: Path('<real_tmpdir>'), while the process cwd is elsewhere
# CPython resolves symlinked temp dirs in os.getcwd(), so compare resolved paths.
resolved_root = root.resolve()
original = os.getcwd()
os.chdir(root)
try:
    # === getcwd / Path.cwd ===
    assert os.chdir(root) is None
    assert Path(os.getcwd()) == resolved_root
    assert Path.cwd() == resolved_root
    assert isinstance(os.getcwd(), str)

    # === relative paths resolve against the working directory ===
    assert open('hello.txt').read() == 'hello world\n'
    assert Path('subdir/nested.txt').read_text() == 'nested content'
    assert Path('./subdir/./deep/file.txt').read_text() == 'deep file'
    assert Path('hello.txt').exists()
    assert Path('missing.txt').exists() is False
    assert Path('x').absolute() == Path.cwd() / 'x'
    assert Path('subdir/../hello.txt').resolve() == resolved_root / 'hello.txt'
    assert sorted(os.listdir()) == ['data.bin', 'empty.txt', 'hello.txt', 'readonly.txt', 'subdir']
    assert sorted(os.listdir('.')) == sorted(os.listdir(root))
    with open('out.txt', 'w') as f:
        f.write('written')
    assert (root / 'out.txt').read_text() == 'written'
    os.mkdir('made')
    assert (root / 'made').is_dir()
    os.rename('out.txt', 'made/moved.txt')
    assert Path('made/moved.txt').read_text() == 'written'

    # === chdir into a subdirectory ===
    os.chdir('subdir')
    assert Path.cwd() == resolved_root / 'subdir'
    assert Path('nested.txt').read_text() == 'nested content'
    assert Path('deep/file.txt').read_text() == 'deep file'
    assert Path('..').resolve() == resolved_root
    assert Path('../hello.txt').read_text() == 'hello world\n'
    os.chdir(Path('deep'))
    assert Path.cwd() == resolved_root / 'subdir' / 'deep'
    os.chdir('../..')
    assert Path.cwd() == resolved_root
    os.chdir('.')
    assert Path.cwd() == resolved_root

    # === chdir errors ===
    # Windows CPython reports WinError messages instead of POSIX errno text.
    try:
        os.chdir('hello.txt')
        assert False, 'expected NotADirectoryError'
    except NotADirectoryError as e:
        if not is_windows:
            assert str(e) == "[Errno 20] Not a directory: 'hello.txt'"
    assert Path.cwd() == resolved_root
    try:
        os.chdir(root / 'missing')
        assert False, 'expected FileNotFoundError'
    except FileNotFoundError as e:
        if not is_windows:
            assert str(e) == f"[Errno 2] No such file or directory: '{root / 'missing'}'"
    assert Path.cwd() == resolved_root
    # Windows CPython raises a different OSError subclass for the empty path.
    try:
        os.chdir('')
        assert False, 'expected FileNotFoundError'
    except OSError as e:
        if not is_windows:
            assert isinstance(e, FileNotFoundError)
            assert str(e) == "[Errno 2] No such file or directory: ''"
    assert Path.cwd() == resolved_root
    try:
        os.chdir(1.5)
        assert False, 'expected TypeError'
    except TypeError as e:
        if not is_windows:
            assert str(e) == 'chdir: path should be string, bytes, os.PathLike or integer, not float'
finally:
    os.chdir(original)
