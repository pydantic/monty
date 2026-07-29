# Tests for os module constants, os.fspath, and argument/type errors that
# raise before any OS access. Filesystem behavior is covered by
# mount_fs__ops.py; getenv/environ by import__os.py / os__environ.py.
import os
import sys
from pathlib import Path

# === constants ===
# Monty always presents the POSIX values regardless of host OS
# (sys.platform == 'monty'); Windows CPython differs (sep='\\',
# linesep='\r\n', name='nt', ...), so skip the asserts there.
if sys.platform != 'win32':
    assert os.sep == '/'
    assert os.altsep is None
    assert os.extsep == '.'
    assert os.curdir == '.'
    assert os.pardir == '..'
    assert os.linesep == '\n'
    assert os.name == 'posix'
    assert os.devnull == '/dev/null'
    # str(Path('/x/y')) is '\\x\\y' on Windows CPython, so this stays guarded.
    assert os.fspath(Path('/x/y')) == '/x/y'

# === fspath ===
assert os.fspath('abc') == 'abc'
assert os.fspath('') == ''
assert os.fspath(b'abc') == b'abc'
assert os.fspath(Path('abc')) == 'abc'
assert os.fspath(path='kw') == 'kw'

try:
    os.fspath(5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'
try:
    os.fspath(None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not NoneType'
try:
    os.fspath()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "fspath() missing required argument 'path' (pos 1)"
try:
    os.fspath('a', 'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'fspath() takes at most 1 argument (2 given)'
try:
    # no positionals at all pivots the wording to "keyword argument"
    os.fspath(path='a', foo=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'fspath() takes at most 1 keyword argument (2 given)'

# === listdir argument errors ===
try:
    os.listdir(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    # Windows CPython has no fd-based listdir, so its converter message omits
    # 'integer'; Monty always uses the POSIX wording.
    if sys.platform == 'win32':
        assert str(e) == 'listdir: path should be string, bytes, os.PathLike or None, not float'
    else:
        assert str(e) == 'listdir: path should be string, bytes, os.PathLike, integer or None, not float'
try:
    os.listdir('.', '.')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'listdir() takes at most 1 argument (2 given)'
try:
    os.listdir('.', foo=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'listdir() takes at most 1 argument (2 given)'
try:
    os.listdir(foo=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "listdir() got an unexpected keyword argument 'foo'"
try:
    os.listdir(path='.', foo=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'listdir() takes at most 1 keyword argument (2 given)'

# === stat argument errors ===
try:
    os.stat()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "stat() missing required argument 'path' (pos 1)"
try:
    os.stat(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'stat: path should be string, bytes, os.PathLike or integer, not float'
try:
    os.stat(path=1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'stat: path should be string, bytes, os.PathLike or integer, not float'
try:
    os.stat('.', foo=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "stat() got an unexpected keyword argument 'foo'"
try:
    os.stat(foo=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "stat() missing required argument 'path' (pos 1)"
try:
    os.stat('.', True)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'stat() takes exactly 1 positional argument (2 given)'
try:
    # the "(N given)" figure counts positionals only, not keywords
    os.stat('.', 1, dir_fd=None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'stat() takes exactly 1 positional argument (2 given)'
try:
    os.stat('.', dir_fd=2**100)
    assert False, 'expected OverflowError'
except OverflowError as e:
    assert str(e) == 'fd is greater than maximum'
try:
    os.stat('.', dir_fd=-(2**100))
    assert False, 'expected OverflowError'
except OverflowError as e:
    assert str(e) == 'fd is less than minimum'
try:
    os.stat('.', 1, 2, 3)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'stat() takes at most 3 arguments (4 given)'
try:
    os.stat('.', path='.')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "argument for stat() given by name ('path') and position (1)"

# === mkdir argument errors ===
try:
    os.mkdir()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "mkdir() missing required argument 'path' (pos 1)"
try:
    os.mkdir(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'mkdir: path should be string, bytes or os.PathLike, not float'
try:
    os.mkdir('a', 'rw')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'str' object cannot be interpreted as an integer"
try:
    # mode is converted through C int before any filesystem access
    os.mkdir('zz-should-not-exist', 2**31)
    assert False, 'expected OverflowError'
except OverflowError as e:
    assert str(e) == 'Python int too large to convert to C int'
try:
    os.mkdir('zz-should-not-exist', 2**100)
    assert False, 'expected OverflowError'
except OverflowError as e:
    assert str(e) == 'Python int too large to convert to C int'
try:
    os.mkdir('a', 1, 2)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'mkdir() takes at most 2 positional arguments (3 given)'
try:
    os.mkdir('a', 1, 2, 3)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'mkdir() takes at most 3 arguments (4 given)'
try:
    os.mkdir(1.5, foo=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "mkdir() got an unexpected keyword argument 'foo'"

# === makedirs argument errors ===
try:
    os.makedirs()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "makedirs() missing 1 required positional argument: 'name'"
try:
    os.makedirs(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not float'
try:
    os.makedirs('a', 1, True, 4)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'makedirs() takes from 1 to 3 positional arguments but 4 were given'
try:
    os.makedirs('a', 1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'float' object cannot be interpreted as an integer"
try:
    os.makedirs('zz-should-not-exist', 2**100)
    assert False, 'expected OverflowError'
except OverflowError as e:
    assert str(e) == 'Python int too large to convert to C int'
try:
    os.makedirs('a', wrong=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "makedirs() got an unexpected keyword argument 'wrong'"

# === remove / unlink / rmdir argument errors ===
try:
    os.remove()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "remove() missing required argument 'path' (pos 1)"
try:
    os.remove(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'remove: path should be string, bytes or os.PathLike, not float'
try:
    os.remove('a', 'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'remove() takes exactly 1 positional argument (2 given)'
try:
    os.unlink(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'unlink: path should be string, bytes or os.PathLike, not float'
try:
    os.unlink('a', 'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'unlink() takes exactly 1 positional argument (2 given)'
try:
    os.rmdir(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'rmdir: path should be string, bytes or os.PathLike, not float'
try:
    os.rmdir('a', 'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'rmdir() takes exactly 1 positional argument (2 given)'

# === rename / replace argument errors ===
try:
    os.rename()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "rename() missing required argument 'src' (pos 1)"
try:
    os.rename('a')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "rename() missing required argument 'dst' (pos 2)"
try:
    os.rename(1.5, 'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'rename: src should be string, bytes or os.PathLike, not float'
try:
    os.rename('a', 1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'rename: dst should be string, bytes or os.PathLike, not float'
try:
    os.rename('a', 'b', 'c')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'rename() takes exactly 2 positional arguments (3 given)'
try:
    os.rename('a', 'b', 'c', src_dir_fd=None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'rename() takes exactly 2 positional arguments (3 given)'
try:
    os.rename('/no-a', '/no-b', src_dir_fd=2**100)
    assert False, 'expected OverflowError'
except OverflowError as e:
    assert str(e) == 'fd is greater than maximum'
try:
    os.rename('a', 'b', dst='c')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "argument for rename() given by name ('dst') and position (2)"
try:
    os.replace()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "replace() missing required argument 'src' (pos 1)"
try:
    os.replace('a')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "replace() missing required argument 'dst' (pos 2)"
try:
    os.replace(1.5, 'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'replace: src should be string, bytes or os.PathLike, not float'
try:
    os.replace('a', 'b', 'c')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'replace() takes exactly 2 positional arguments (3 given)'
