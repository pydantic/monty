# mount-fs
# skip-cpython-windows — os.path is ntpath on Windows CPython; Monty's is always posixpath
# Filesystem-backed os.path functions against the mounted test directory.
import os
from pathlib import Path

# root is injected by the test runner: Path('/mnt') for Monty (also the
# working directory), the real temp dir for CPython.
hello = root / 'hello.txt'

# === exists / isfile / isdir / islink ===
assert os.path.exists(hello) == True
assert os.path.exists(str(hello)) == True
assert os.path.exists(root / 'subdir') == True
assert os.path.exists(root / 'nope') == False
assert os.path.exists(root / 'nope' / 'deeper') == False
assert os.path.exists(path=hello) == True
assert os.path.isfile(hello) == True
assert os.path.isfile(root / 'subdir') == False
assert os.path.isfile(root / 'nope') == False
assert os.path.isdir(root / 'subdir') == True
assert os.path.isdir(s=root) == True
assert os.path.isdir(hello) == False
assert os.path.isdir(root / 'nope') == False
assert os.path.islink(hello) == False
assert os.path.islink(root / 'nope') == False
# a NUL byte makes the predicates answer False without consulting the host
assert os.path.exists(str(root) + '/\0') == False
assert os.path.isdir(str(root) + '/\0') == False

# === lexists / ismount ===
assert os.path.lexists(hello) == True
assert os.path.lexists(path=root / 'subdir') == True
assert os.path.lexists(root / 'nope') == False
assert os.path.lexists(str(root) + '/\0') == False
# Monty reports every existing path as a mount point; a missing one is not
assert os.path.ismount(root / 'nope') == False
assert os.path.ismount(str(root) + '/\0') == False
assert isinstance(os.path.ismount(root), bool)

# === getsize / getmtime / getatime / getctime ===
assert os.path.getsize(hello) == 12
assert os.path.getsize(filename=root / 'empty.txt') == 0
assert os.path.getsize(str(root / 'subdir' / 'nested.txt')) == 14
assert isinstance(os.path.getmtime(hello), float)
assert isinstance(os.path.getatime(hello), float)
assert isinstance(os.path.getctime(hello), float)
assert os.path.getmtime(hello) == os.stat(hello).st_mtime
try:
    os.path.getsize(root / 'nope')
    assert False, 'expected FileNotFoundError'
except FileNotFoundError as e:
    assert str(e) == f"[Errno 2] No such file or directory: '{root / 'nope'}'"
try:
    os.path.getsize(str(root) + '/\0')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'stat: embedded null character in path'

# === samestat / samefile ===
# mounts report no inode or device numbers, so only the same-file case holds on both engines
assert os.path.samestat(os.stat(hello), os.stat(hello)) == True
# samefile falls back to comparing normalized paths when there are no inodes
assert os.path.samefile(hello, hello) == True
assert os.path.samefile(hello, root / 'subdir' / '..' / 'hello.txt') == True
assert os.path.samefile(f1=str(hello), f2=hello) == True
assert os.path.samefile(hello, root / 'empty.txt') == False
assert os.path.samefile(root, root / 'subdir') == False
try:
    os.path.samefile(root / 'nope', hello)
    assert False, 'expected FileNotFoundError'
except FileNotFoundError as e:
    assert str(e) == f"[Errno 2] No such file or directory: '{root / 'nope'}'"
try:
    os.path.samefile(hello, root / 'nope')
    assert False, 'expected FileNotFoundError'
except FileNotFoundError as e:
    assert str(e) == f"[Errno 2] No such file or directory: '{root / 'nope'}'"
# the second argument is only checked once the first stat succeeded
try:
    os.path.samefile(root / 'nope', 1.5)
    assert False, 'expected FileNotFoundError'
except FileNotFoundError as e:
    assert str(e) == f"[Errno 2] No such file or directory: '{root / 'nope'}'"
try:
    os.path.samefile(hello, 1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'stat: path should be string, bytes, os.PathLike or integer, not float'
try:
    os.path.samefile(hello, '')
    assert False, 'expected FileNotFoundError'
except FileNotFoundError as e:
    assert str(e) == "[Errno 2] No such file or directory: ''"
try:
    os.path.samefile(hello, str(root) + '/\0')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'stat: embedded null character in path'

# === realpath ===
# resolves like Path.resolve() on both engines (the temp dir may sit behind a symlink on macOS)
assert os.path.realpath(hello) == str(hello.resolve())
assert os.path.realpath(root / 'subdir' / '..' / 'hello.txt') == str(hello.resolve())
assert os.path.realpath(filename=root / 'nope') == str((root / 'nope').resolve())
assert isinstance(os.path.realpath(root), str)
assert os.path.realpath(hello, strict=True) == str(hello.resolve())
assert os.path.realpath(root / 'subdir' / '..' / 'hello.txt', strict=1) == str(hello.resolve())
assert os.path.realpath(root / 'nope', strict=False) == str((root / 'nope').resolve())
try:
    os.path.realpath(root / 'nope', strict=True)
    assert False, 'expected FileNotFoundError'
except FileNotFoundError as e:
    assert str(e) == f"[Errno 2] No such file or directory: '{(root / 'nope').resolve()}'"
try:
    os.path.realpath(str(root) + '/\0')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'lstat: embedded null character in path'

# === relative paths resolve against the working directory ===
original = os.getcwd()
os.chdir(root)
try:
    assert os.path.exists('hello.txt') == True
    assert os.path.isfile('subdir/nested.txt') == True
    assert os.path.isdir('subdir/deep') == True
    assert os.path.getsize('subdir/deep/file.txt') == 9
    assert os.path.realpath('subdir/../hello.txt') == str(hello.resolve())
    assert os.path.realpath('hello.txt', strict=True) == str(hello.resolve())
    assert os.path.samefile('hello.txt', hello) == True
    assert os.path.lexists('subdir') == True
    assert os.path.abspath('hello.txt') == os.path.join(os.getcwd(), 'hello.txt')
finally:
    os.chdir(original)
