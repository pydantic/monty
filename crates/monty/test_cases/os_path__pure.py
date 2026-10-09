# call-external
# skip-cpython-windows — os.path is ntpath on Windows CPython; Monty's is always posixpath
# Pure os.path functions (no filesystem), plus expanduser/expandvars which
# consult the host environment. Filesystem-backed functions are in os_path__fs.py.
import os
import os.path
import posixpath
from os import path as os_path_alias
from os.path import join, splitext
from pathlib import Path

# === import forms ===
assert os.path is not None
assert posixpath.join('a', 'b') == 'a/b'
assert os_path_alias.join('a', 'b') == 'a/b'
assert join('a', 'b') == 'a/b'
assert splitext('a.b') == ('a', '.b')
import os.path as osp

assert osp.join('a', 'b') == 'a/b'

# === constants ===
assert os.path.sep == '/'
assert os.path.altsep is None
assert os.path.extsep == '.'
assert os.path.curdir == '.'
assert os.path.pardir == '..'
assert os.path.pathsep == ':'
assert os.path.defpath == '/bin:/usr/bin'
assert os.path.devnull == '/dev/null'
assert os.pathsep == ':'
assert os.defpath == '/bin:/usr/bin'
assert isinstance(os.path.supports_unicode_filenames, bool)

# === join ===
assert os.path.join('a') == 'a'
assert os.path.join('a', 'b') == 'a/b'
assert os.path.join('a/', 'b') == 'a/b'
assert os.path.join('a', '/b') == '/b'
assert os.path.join('', 'b') == 'b'
assert os.path.join('a', '') == 'a/'
assert os.path.join('a', '', 'b') == 'a/b'
assert os.path.join('/a', 'b', 'c') == '/a/b/c'
assert os.path.join('a', 'b', '/c', 'd') == '/c/d'
assert os.path.join('', '') == ''
assert os.path.join(a='x') == 'x'
assert os.path.join(Path('a'), 'b') == 'a/b'
assert os.path.join('a', Path('b')) == 'a/b'
assert os.path.join(b'a', b'b') == b'a/b'
assert os.path.join(b'a', b'/b', b'c') == b'/b/c'

try:
    os.path.join(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'
try:
    os.path.join('a', 1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "join() argument must be str, bytes, or os.PathLike object, not 'int'"
try:
    os.path.join('a', b'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "Can't mix strings and bytes in path components"
try:
    os.path.join(b'a', 'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "Can't mix strings and bytes in path components"
try:
    os.path.join('a', b'b', 1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "join() argument must be str, bytes, or os.PathLike object, not 'int'"
# CPython re-checks the raw arguments, where a Path is neither str nor bytes
try:
    os.path.join('a', Path('b'), 1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "join() argument must be str, bytes, or os.PathLike object, not 'PosixPath'"
try:
    os.path.join(b'a', Path('b'))
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "join() argument must be str, bytes, or os.PathLike object, not 'PosixPath'"
try:
    os.path.join()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "join() missing 1 required positional argument: 'a'"
try:
    os.path.join('a', b='x')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "join() got an unexpected keyword argument 'b'"

# === split / basename / dirname ===
assert os.path.split('') == ('', '')
assert os.path.split('/') == ('/', '')
assert os.path.split('//') == ('//', '')
assert os.path.split('a') == ('', 'a')
assert os.path.split('/a') == ('/', 'a')
assert os.path.split('a/') == ('a', '')
assert os.path.split('a/b') == ('a', 'b')
assert os.path.split('//a//b//') == ('//a//b', '')
assert os.path.split('/a//') == ('/a', '')
assert os.path.split(p='a/b') == ('a', 'b')
assert os.path.split(Path('a/b')) == ('a', 'b')
assert os.path.split(b'a/b') == (b'a', b'b')
assert os.path.basename('') == ''
assert os.path.basename('/') == ''
assert os.path.basename('a/') == ''
assert os.path.basename('/a') == 'a'
assert os.path.basename('a/b') == 'b'
assert os.path.basename(b'a/b') == b'b'
assert os.path.dirname('') == ''
assert os.path.dirname('/') == '/'
assert os.path.dirname('//') == '//'
assert os.path.dirname('a') == ''
assert os.path.dirname('/a') == '/'
assert os.path.dirname('a/') == 'a'
assert os.path.dirname('//a//b//') == '//a//b'
assert os.path.dirname('/a//') == '/a'
assert os.path.dirname(b'/a/b') == b'/a'

try:
    os.path.split(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'
try:
    os.path.split()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "split() missing 1 required positional argument: 'p'"
try:
    os.path.split('a', 'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'split() takes 1 positional argument but 2 were given'
try:
    os.path.basename(None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not NoneType'
try:
    os.path.dirname(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not float'

# === splitext ===
assert os.path.splitext('') == ('', '')
assert os.path.splitext('a') == ('a', '')
assert os.path.splitext('a.b') == ('a', '.b')
assert os.path.splitext('.b') == ('.b', '')
assert os.path.splitext('a.') == ('a', '.')
assert os.path.splitext('..b') == ('..b', '')
assert os.path.splitext('a..b') == ('a.', '.b')
assert os.path.splitext('a/.b') == ('a/.b', '')
assert os.path.splitext('a.b/c') == ('a.b/c', '')
assert os.path.splitext('.a.b') == ('.a', '.b')
assert os.path.splitext('...') == ('...', '')
assert os.path.splitext('a/b.c.d') == ('a/b.c', '.d')
assert os.path.splitext('/.') == ('/.', '')
assert os.path.splitext('a/..b') == ('a/..b', '')
assert os.path.splitext(b'a.b') == (b'a', b'.b')
assert os.path.splitext(Path('x/y.tar.gz')) == ('x/y.tar', '.gz')
try:
    os.path.splitext(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'

# === splitdrive / splitroot ===
assert os.path.splitdrive('/x') == ('', '/x')
assert os.path.splitdrive(b'x') == (b'', b'x')
assert os.path.splitroot('') == ('', '', '')
assert os.path.splitroot('/') == ('', '/', '')
assert os.path.splitroot('//') == ('', '//', '')
assert os.path.splitroot('///') == ('', '/', '//')
assert os.path.splitroot('//a') == ('', '//', 'a')
assert os.path.splitroot('///a') == ('', '/', '//a')
assert os.path.splitroot('a') == ('', '', 'a')
assert os.path.splitroot(p='a') == ('', '', 'a')
assert os.path.splitroot(b'//a') == (b'', b'//', b'a')
try:
    os.path.splitroot(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == '_path_splitroot_ex: path should be string, bytes or os.PathLike, not int'
try:
    os.path.splitroot()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "_path_splitroot_ex() missing required argument 'p' (pos 1)"
try:
    os.path.splitroot('a', 'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == '_path_splitroot_ex() takes at most 1 argument (2 given)'

# === isabs / normcase ===
assert os.path.isabs('') == False
assert os.path.isabs('/') == True
assert os.path.isabs('a') == False
assert os.path.isabs('//a') == True
assert os.path.isabs(b'/x') == True
assert os.path.isabs(b'') == False
assert os.path.isabs(Path('/x')) == True
assert os.path.isabs(Path('x')) == False
assert os.path.isabs(s='/') == True
assert os.path.normcase('/A/b') == '/A/b'
assert os.path.normcase(b'A') == b'A'
assert os.path.normcase(Path('/x')) == '/x'
try:
    os.path.isabs(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'
try:
    os.path.normcase(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'

# === normpath ===
assert os.path.normpath('') == '.'
assert os.path.normpath('/') == '/'
assert os.path.normpath('//') == '//'
assert os.path.normpath('///') == '/'
assert os.path.normpath('//a') == '//a'
assert os.path.normpath('///a') == '/a'
assert os.path.normpath('a//b/./c/../d') == 'a/b/d'
assert os.path.normpath('../a') == '../a'
assert os.path.normpath('/../a') == '/a'
assert os.path.normpath('a/../..') == '..'
assert os.path.normpath('.') == '.'
assert os.path.normpath('/.') == '/'
assert os.path.normpath('a/') == 'a'
assert os.path.normpath('/a/') == '/a'
assert os.path.normpath('a/b/../../..') == '..'
assert os.path.normpath('..') == '..'
assert os.path.normpath('/..') == '/'
assert os.path.normpath('//..') == '//'
assert os.path.normpath('a/../../b') == '../b'
assert os.path.normpath('/a/../../b') == '/b'
assert os.path.normpath('./a/.') == 'a'
assert os.path.normpath('a\0b') == 'a\x00b'
assert os.path.normpath(path='a//b') == 'a/b'
assert os.path.normpath(b'a//b') == b'a/b'
assert os.path.normpath(Path('a//b')) == 'a/b'
try:
    os.path.normpath(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == '_path_normpath: path should be string, bytes or os.PathLike, not int'
try:
    os.path.normpath(None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == '_path_normpath: path should be string, bytes or os.PathLike, not NoneType'
try:
    os.path.normpath()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "_path_normpath() missing required argument 'path' (pos 1)"
try:
    os.path.normpath('a', 'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == '_path_normpath() takes at most 1 argument (2 given)'
try:
    os.path.normpath(path='a', x='b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == '_path_normpath() takes at most 1 keyword argument (2 given)'

# === abspath ===
# relative inputs resolve against the working directory on both engines
cwd = os.getcwd()
assert os.path.abspath('/a/../b') == '/b'
assert os.path.abspath('/') == '/'
assert os.path.abspath('//a') == '//a'
assert os.path.abspath('') == cwd
assert os.path.abspath('.') == cwd
assert os.path.abspath('a/./b/../c') == os.path.join(cwd, 'a/c')
assert os.path.abspath(b'a') == os.path.join(os.getcwdb(), b'a')
assert os.path.abspath(Path('/x/y/..')) == '/x'
try:
    os.path.abspath(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'

# === relpath ===
assert os.path.relpath('/a/b', '/a') == 'b'
assert os.path.relpath('/a', '/a/b/c') == '../..'
assert os.path.relpath('/a', '/a') == '.'
assert os.path.relpath('/a/b/c', '/a/d') == '../b/c'
assert os.path.relpath('a', 'a/b') == '..'
assert os.path.relpath('a') == 'a'
assert os.path.relpath('a', None) == 'a'
assert os.path.relpath('a', '') == 'a'
assert os.path.relpath('/', '/') == '.'
assert os.path.relpath('//a', '/') == 'a'
assert os.path.relpath(path='/a', start='/') == 'a'
assert os.path.relpath(b'/a/b', b'/a') == b'b'
assert os.path.relpath(Path('/a/b'), Path('/a')) == 'b'
assert os.path.relpath(os.path.join(cwd, 'x')) == 'x'
try:
    os.path.relpath('')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'no path specified'
try:
    os.path.relpath(b'', 'x')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'no path specified'
try:
    os.path.relpath(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'
try:
    os.path.relpath('a', 1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'
try:
    os.path.relpath('a', b'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "Can't mix strings and bytes in path components"
try:
    os.path.relpath(b'a', 'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "Can't mix strings and bytes in path components"
try:
    os.path.relpath()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "relpath() missing 1 required positional argument: 'path'"
try:
    os.path.relpath('a', 'b', 'c')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'relpath() takes from 1 to 2 positional arguments but 3 were given'

# === commonpath ===
assert os.path.commonpath(['/a/b/c', '/a/b/d']) == '/a/b'
assert os.path.commonpath(['a/b/c', 'a/./b/d']) == 'a/b'
assert os.path.commonpath(['/a', '/b']) == '/'
assert os.path.commonpath(['a', 'b']) == ''
assert os.path.commonpath(['/a//b/', '/a/b']) == '/a/b'
assert os.path.commonpath(['/a', '/a/', '/a/b']) == '/a'
assert os.path.commonpath(['//a/b', '//a/c']) == '/a'
assert os.path.commonpath(['a/..', 'a/b']) == 'a'
assert os.path.commonpath(['/', '/a']) == '/'
assert os.path.commonpath(['']) == ''
assert os.path.commonpath(['', 'a']) == ''
assert os.path.commonpath('abc') == ''
assert os.path.commonpath(('a', 'a/b')) == 'a'
assert os.path.commonpath(iter(['a'])) == 'a'
assert os.path.commonpath(paths=['/x/y', '/x']) == '/x'
assert os.path.commonpath([Path('/a/b'), '/a/c']) == '/a'
assert os.path.commonpath([b'/a/b', b'/a/c']) == b'/a'
try:
    os.path.commonpath([])
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'commonpath() arg is an empty sequence'
try:
    os.path.commonpath(['/a', 'b'])
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "Can't mix absolute and relative paths"
try:
    os.path.commonpath([1])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'
try:
    os.path.commonpath(['a', 'b', 1])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'
try:
    os.path.commonpath(['/a', b'/b'])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "Can't mix strings and bytes in path components"
try:
    os.path.commonpath([b'/a', '/b'])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "Can't mix strings and bytes in path components"
try:
    os.path.commonpath(5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'int' object is not iterable"

# === commonprefix ===
assert os.path.commonprefix([]) == ''
assert os.path.commonprefix(['abc', 'abd']) == 'ab'
assert os.path.commonprefix(['a', 'a']) == 'a'
assert os.path.commonprefix(['', 'a']) == ''
assert os.path.commonprefix(['é1', 'é2']) == 'é'
assert os.path.commonprefix('abc') == ''
assert os.path.commonprefix(('ab',)) == 'ab'
assert os.path.commonprefix(m=['xy', 'xz']) == 'x'
assert os.path.commonprefix([b'abc', b'abd']) == b'ab'
assert os.path.commonprefix([Path('a/b'), 'a/c']) == 'a/'
assert os.path.commonprefix(['ab', Path('ab')]) == 'ab'
assert os.path.commonprefix([[1, 2], [1, 3]]) == [1]
assert os.path.commonprefix([(1, 2), (1, 2, 3)]) == (1, 2)
assert os.path.commonprefix([['a'], ['a', 'b']]) == ['a']
assert os.path.commonprefix([['a', 'b'], ['a', 'b', 'c']]) == ['a', 'b']
assert os.path.commonprefix([[1, 2], [3]]) == []
assert os.path.commonprefix([[], [1]]) == []
assert os.path.commonprefix([[1.0, 2], [1, 3]]) == [1.0]
try:
    os.path.commonprefix(5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'int' object is not subscriptable"
try:
    os.path.commonprefix([1])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'
try:
    os.path.commonprefix(['a', 1])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'
try:
    os.path.commonprefix(['a', b'b'])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'<' not supported between instances of 'bytes' and 'str'"
try:
    os.path.commonprefix([b'b', 'a'])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'<' not supported between instances of 'str' and 'bytes'"
try:
    os.path.commonprefix([[1, 2], 'ab'])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'<' not supported between instances of 'str' and 'list'"
try:
    os.path.commonprefix([(1,), [1]])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'<' not supported between instances of 'list' and 'tuple'"
try:
    os.path.commonprefix()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "commonprefix() missing 1 required positional argument: 'm'"

# === isjunction / isdevdrive ===
assert os.path.isjunction('x') == False
assert os.path.isjunction(Path('/')) == False
assert os.path.isdevdrive(b'/') == False
try:
    os.path.isjunction(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'
try:
    os.path.isdevdrive(None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not NoneType'

# === samestat ===
try:
    os.path.samestat(1, 2)
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "'int' object has no attribute 'st_ino'"
try:
    os.path.samestat(s1=1.5, s2=2)
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "'float' object has no attribute 'st_ino'"
try:
    os.path.samestat()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "samestat() missing 2 required positional arguments: 's1' and 's2'"

# === expanduser ===
# `~` and `~/...` need $HOME from the host; `~user` would need the password
# database, which neither the virtual environment nor the sandbox has.
assert os.path.expanduser('') == ''
assert os.path.expanduser('x~') == 'x~'
assert os.path.expanduser('~~') == '~~'
assert os.path.expanduser('~no_such_user_zz/x') == '~no_such_user_zz/x'
assert os.path.expanduser(b'x') == b'x'
assert os.path.expanduser(Path('a/b')) == 'a/b'
assert os.path.expanduser('~/x').endswith('/x')
assert not os.path.expanduser('~').endswith('/'), 'home has no trailing slash'
assert os.path.expanduser(b'~/x').endswith(b'/x')
try:
    os.path.expanduser(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'
try:
    os.path.expanduser('~', 1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expanduser() takes 1 positional argument but 2 were given'

# === expandvars ===
# the virtual environment holds VIRTUAL_HOME, VIRTUAL_USER and VIRTUAL_EMPTY
assert os.path.expandvars('no dollars') == 'no dollars'
assert os.path.expandvars('$VIRTUAL_HOME/x') == '/virtual/home/x'
assert os.path.expandvars('${VIRTUAL_HOME}/x') == '/virtual/home/x'
assert os.path.expandvars('$VIRTUAL_USER$VIRTUAL_USER') == 'testusertestuser'
assert os.path.expandvars('a$VIRTUAL_EMPTY/b') == 'a/b'
assert os.path.expandvars('$VIRTUAL_HOMEé') == '/virtual/homeé'
assert os.path.expandvars('${VIRTUAL_HOME}}') == '/virtual/home}'
assert os.path.expandvars('$$VIRTUAL_USER') == '$testuser'
assert os.path.expandvars('${VIRTUAL_HOME') == '${VIRTUAL_HOME'
assert os.path.expandvars('${}') == '${}'
assert os.path.expandvars('$') == '$'
assert os.path.expandvars('$é') == '$é'
assert os.path.expandvars('${{VIRTUAL_HOME}') == '${{VIRTUAL_HOME}'
assert os.path.expandvars('${NOPE X}') == '${NOPE X}'
assert os.path.expandvars('$NONEXISTENT/x') == '$NONEXISTENT/x'
assert os.path.expandvars('${NONEXISTENT}') == '${NONEXISTENT}'
assert os.path.expandvars(Path('$VIRTUAL_USER/x')) == 'testuser/x'
# bytes paths read `os.environb`, which the virtual environment does not cover
assert os.path.expandvars(b'$NONEXISTENT_ZZ/\xff') == b'$NONEXISTENT_ZZ/\xff'
assert os.path.expandvars(b'no dollars') == b'no dollars'
try:
    os.path.expandvars(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'

# === realpath (pure cases) ===
assert os.path.realpath('') == cwd
assert os.path.realpath('', strict=True) == cwd
try:
    os.path.realpath(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'expected str, bytes or os.PathLike object, not int'
try:
    os.path.realpath()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "realpath() missing 1 required positional argument: 'filename'"
try:
    os.path.realpath('x', True)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'realpath() takes 1 positional argument but 2 were given'

# === host-backed predicates: type errors and the empty path need no host ===
assert os.path.exists('') == False
assert os.path.isdir('') == False
assert os.path.isfile('') == False
assert os.path.islink('') == False
assert os.path.lexists('') == False
assert os.path.ismount('') == False
try:
    os.path.lexists(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'lstat: path should be string, bytes or os.PathLike, not float'
try:
    os.path.ismount(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'lstat: path should be string, bytes or os.PathLike, not float'
try:
    os.path.ismount()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "ismount() missing 1 required positional argument: 'path'"
try:
    os.path.samefile(1.5, 'x')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'stat: path should be string, bytes, os.PathLike or integer, not float'
try:
    os.path.samefile('', 'x')
    assert False, 'expected FileNotFoundError'
except FileNotFoundError as e:
    assert str(e) == "[Errno 2] No such file or directory: ''"
try:
    os.path.samefile('x')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "samefile() missing 1 required positional argument: 'f2'"
try:
    os.path.exists(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'stat: path should be string, bytes, os.PathLike or integer, not float'
try:
    os.path.exists(None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'stat: path should be string, bytes, os.PathLike or integer, not NoneType'
try:
    os.path.isfile(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'stat: path should be string, bytes, os.PathLike or integer, not float'
try:
    os.path.isdir(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'stat: path should be string, bytes, os.PathLike or integer, not float'
try:
    os.path.islink(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'lstat: path should be string, bytes or os.PathLike, not float'
try:
    os.path.getsize(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'stat: path should be string, bytes, os.PathLike or integer, not float'
try:
    os.path.getmtime(None)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'stat: path should be string, bytes, os.PathLike or integer, not NoneType'
try:
    os.path.getsize('')
    assert False, 'expected FileNotFoundError'
except FileNotFoundError as e:
    assert str(e) == "[Errno 2] No such file or directory: ''"
try:
    os.path.exists()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "exists() missing 1 required positional argument: 'path'"
