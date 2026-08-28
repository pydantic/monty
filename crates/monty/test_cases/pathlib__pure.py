# skip-cpython-windows — pathlib uses POSIX paths in Monty's sandbox, Windows CPython resolves differently
# === Path constructor ===
from pathlib import Path

p = Path('/usr/local/bin/python')
assert str(p) == '/usr/local/bin/python'

# Constructor with multiple arguments
assert str(Path('folder', 'file.txt')) == 'folder/file.txt'
assert str(Path('/usr', 'local', 'bin')) == '/usr/local/bin'
assert str(Path('start', '/absolute', 'end')) == '/absolute/end'

# Constructor with no arguments
assert str(Path()) == '.'

# === name property ===
assert p.name == 'python'
assert Path('/usr/local/bin/').name == 'bin'
assert Path('/').name == ''
assert Path('file.txt').name == 'file.txt'

# === parent property ===
assert str(p.parent) == '/usr/local/bin'
assert str(Path('/usr').parent) == '/'
assert str(Path('/').parent) == '/'
assert str(Path('file.txt').parent) == '.'

# === stem property ===
assert Path('/path/file.tar.gz').stem == 'file.tar'
assert Path('/path/file.txt').stem == 'file'
assert Path('/path/.bashrc').stem == '.bashrc'
assert Path('/path/file').stem == 'file'

# === suffix property ===
assert Path('/path/file.tar.gz').suffix == '.gz'
assert Path('/path/file.txt').suffix == '.txt'
assert Path('/path/.bashrc').suffix == ''
assert Path('/path/file').suffix == ''

# === suffixes property ===
assert Path('/path/file.tar.gz').suffixes == ['.tar', '.gz']
assert Path('/path/file.txt').suffixes == ['.txt']
assert Path('/path/.bashrc').suffixes == []

# === parts property ===
assert Path('/usr/local/bin').parts == ('/', 'usr', 'local', 'bin')
assert Path('usr/local').parts == ('usr', 'local')
assert Path('/').parts == ('/',)

# === is_absolute method ===
assert Path('/usr/bin').is_absolute() == True
assert Path('usr/bin').is_absolute() == False
assert Path('').is_absolute() == False

# === joinpath method ===
assert str(Path('/usr').joinpath('local')) == '/usr/local'
assert str(Path('/usr').joinpath('local', 'bin')) == '/usr/local/bin'
assert str(Path('/usr').joinpath('/etc')) == '/etc'
assert str(Path('.').joinpath('file')) == 'file'

# === with_name method ===
assert str(Path('/path/file.txt').with_name('other.py')) == '/path/other.py'
assert str(Path('file.txt').with_name('other.py')) == 'other.py'

# === with_suffix method ===
assert str(Path('/path/file.txt').with_suffix('.py')) == '/path/file.py'
assert str(Path('/path/file.txt').with_suffix('')) == '/path/file'
assert str(Path('/path/file').with_suffix('.txt')) == '/path/file.txt'

# === / operator ===
assert str(Path('/usr') / 'local') == '/usr/local'
assert str(Path('/usr') / 'local' / 'bin') == '/usr/local/bin'

# Path / Path
assert str(Path('/usr') / Path('bin')) == '/usr/bin'
assert str(Path('/usr') / Path('/etc')) == '/etc'

# Path / heap-allocated (non-interned) str
heap_str = 'lo' + 'cal'
assert str(Path('/usr') / heap_str) == '/usr/local'

# str / Path (__rtruediv__)
assert str('/usr' / Path('bin')) == '/usr/bin'
assert str(heap_str / Path('bin')) == 'local/bin'
assert str('/usr' / Path('/etc')) == '/etc'

# absolute rhs replaces lhs
assert str(Path('/usr') / '/etc') == '/etc'

# empty and dot components
assert str(Path('.') / 'a') == 'a'
assert str(Path('/usr') / '') == '/usr'
assert str(Path('/usr') / '.') == '/usr'
assert str('' / Path('a')) == 'a'

# augmented assignment
aug = Path('/a')
aug /= 'b'
assert str(aug) == '/a/b'
aug /= Path('c')
assert str(aug) == '/a/b/c'

# unsupported operand types raise TypeError matching CPython
try:
    Path('/usr') / 1
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for /: 'PosixPath' and 'int'"
try:
    1 / Path('/usr')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for /: 'int' and 'PosixPath'"
try:
    Path('/usr') / b'bin'
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for /: 'PosixPath' and 'bytes'"
try:
    b'/usr' / Path('bin')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for /: 'bytes' and 'PosixPath'"

# === as_posix method ===
assert Path('/usr/bin').as_posix() == '/usr/bin'

# === __fspath__ method (os.PathLike protocol) ===
assert Path('/usr/bin').__fspath__() == '/usr/bin'

# === dot normalization ===
# CPython normalizes '.' components but keeps '..'
assert str(Path('/a/./b')) == '/a/b'
assert str(Path('/a/b/.')) == '/a/b'
assert str(Path('./a')) == 'a'
assert str(Path('.')) == '.'
assert str(Path('/a/b/..')) == '/a/b/..'
assert str(Path('/a/./b/../c')) == '/a/b/../c'
assert str(Path('/a///b')) == '/a/b'

# === repr ===
r = repr(Path('/usr/bin'))
assert r == "PosixPath('/usr/bin')", f'repr should be PosixPath, got {r}'
