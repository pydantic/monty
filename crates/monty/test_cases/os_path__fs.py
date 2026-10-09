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

# === samestat ===
# mounts report no inode or device numbers, so only the same-file case holds on both engines
assert os.path.samestat(os.stat(hello), os.stat(hello)) == True

# === realpath ===
# resolves like Path.resolve() on both engines (the temp dir may sit behind a symlink on macOS)
assert os.path.realpath(hello) == str(hello.resolve())
assert os.path.realpath(root / 'subdir' / '..' / 'hello.txt') == str(hello.resolve())
assert os.path.realpath(filename=root / 'nope') == str((root / 'nope').resolve())
assert isinstance(os.path.realpath(root), str)
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
    assert os.path.abspath('hello.txt') == os.path.join(os.getcwd(), 'hello.txt')
finally:
    os.chdir(original)
