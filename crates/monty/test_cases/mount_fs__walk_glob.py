# mount-fs
# skip-cpython-windows
import os
from collections import namedtuple
from pathlib import Path

# `root` is Path('/mnt') in Monty and a real temp directory in CPython; compare
# paths relative to it. Layout: hello.txt, empty.txt, data.bin, readonly.txt,
# subdir/nested.txt, subdir/deep/file.txt
prefix = len(str(root))


def rel(path):
    return str(path)[prefix:]


def listed(names):
    # the fixture's dangling link under subdir/deep is listed by CPython but hidden by mounts
    return [name for name in names if name != 'dangling']


def globbed(pattern, base=root, **kwargs):
    return sorted(rel(p) for p in base.glob(pattern, **kwargs) if p.name != 'dangling')


# === Path.glob: wildcards ===
top_files = ['/data.bin', '/empty.txt', '/hello.txt', '/readonly.txt']
assert globbed('*.txt') == ['/empty.txt', '/hello.txt', '/readonly.txt']
assert globbed('*') == top_files + ['/subdir']
assert globbed('[eh]*.txt') == ['/empty.txt', '/hello.txt']
assert globbed('[!eh]*.txt') == ['/readonly.txt']
assert globbed('?mpty.*') == ['/empty.txt']
assert globbed('*.TXT') == []
assert globbed('*.TXT', case_sensitive=False) == ['/empty.txt', '/hello.txt', '/readonly.txt']
assert globbed('HELLO.TXT', case_sensitive=False) == ['/hello.txt']
assert globbed('hello.txt', case_sensitive=True) == ['/hello.txt']

# === Path.glob: directories and nesting ===
assert globbed('*/') == ['/subdir']
assert globbed('subdir/*') == ['/subdir/deep', '/subdir/nested.txt']
assert globbed('*/nested.txt') == ['/subdir/nested.txt']
assert globbed('*/*/*') == ['/subdir/deep/file.txt']
assert globbed('*/*/') == ['/subdir/deep']
assert globbed('s*/deep/*') == ['/subdir/deep/file.txt']
assert globbed('subdir/deep/file.txt') == ['/subdir/deep/file.txt']
assert globbed('subdir/') == ['/subdir']
assert globbed('hello.txt') == ['/hello.txt']
assert globbed('hello.txt/') == []
assert globbed('nope.txt') == []
assert globbed('nope/*') == []
assert globbed('*', base=root / 'nope') == []
assert globbed('*', base=root / 'hello.txt') == []

# === Path.glob: recursive ===
all_files = top_files + ['/subdir/deep/file.txt', '/subdir/nested.txt']
assert globbed('**') == sorted([''] + all_files + ['/subdir', '/subdir/deep'])
assert globbed('**/') == ['', '/subdir', '/subdir/deep']
assert globbed('**/*.txt') == [
    '/empty.txt',
    '/hello.txt',
    '/readonly.txt',
    '/subdir/deep/file.txt',
    '/subdir/nested.txt',
]
assert globbed('**/deep/*') == ['/subdir/deep/file.txt']
assert globbed('subdir/**') == ['/subdir', '/subdir/deep', '/subdir/deep/file.txt', '/subdir/nested.txt']
assert globbed('subdir/**/*.txt') == ['/subdir/deep/file.txt', '/subdir/nested.txt']
assert globbed('**/*.txt', recurse_symlinks=True) == globbed('**/*.txt')

# === Path.glob: `..` and the receiver's spelling ===
assert globbed('subdir/../hello.txt') == ['/subdir/../hello.txt']
assert globbed('../*.bin', base=root / 'subdir') == ['/subdir/../data.bin']
assert sorted(p.name for p in (root / 'subdir').glob('*')) == ['deep', 'nested.txt']
assert next(root.glob('hello.*')) == root / 'hello.txt'
assert all(isinstance(p, Path) for p in root.glob('**'))

# === Path.rglob ===
assert sorted(rel(p) for p in root.rglob('*.txt')) == globbed('**/*.txt')
assert sorted(rel(p) for p in root.rglob('')) == ['', '/subdir', '/subdir/deep']
assert sorted(rel(p) for p in root.rglob('deep')) == ['/subdir/deep']
assert sorted(rel(p) for p in root.rglob('file.txt')) == ['/subdir/deep/file.txt']

# === Path.glob: pattern errors ===
try:
    root.glob('')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "Unacceptable pattern: ''"
try:
    root.glob('./')
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == "Unacceptable pattern: './'"
try:
    root.glob('/abs')
    assert False, 'expected NotImplementedError'
except NotImplementedError as e:
    assert str(e) == 'Non-relative patterns are unsupported'
try:
    root.rglob('/abs')
    assert False, 'expected NotImplementedError'
except NotImplementedError as e:
    assert str(e) == 'Non-relative patterns are unsupported'
try:
    root.glob(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == '_path_splitroot_ex: path should be string, bytes or os.PathLike, not int'
try:
    root.glob('*', foo=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "Path.glob() got an unexpected keyword argument 'foo'"

# === os.walk ===
walked = [(rel(dirpath), dirnames, listed(filenames)) for dirpath, dirnames, filenames in os.walk(root)]
assert sorted((d, sorted(dn), sorted(fn)) for d, dn, fn in walked) == [
    ('', ['subdir'], ['data.bin', 'empty.txt', 'hello.txt', 'readonly.txt']),
    ('/subdir', ['deep'], ['nested.txt']),
    ('/subdir/deep', [], ['file.txt']),
]
assert [d for d, _, _ in walked] == ['', '/subdir', '/subdir/deep']
assert [rel(d) for d, _, _ in os.walk(root, topdown=False)] == ['/subdir/deep', '/subdir', '']
assert [rel(d) for d, _, _ in os.walk(str(root) + '/subdir/')] == ['/subdir/', '/subdir/deep']
assert all(type(d) is str for d, _, _ in os.walk(root))

# pruning `dirnames` in place stops the descent
seen = []
for dirpath, dirnames, filenames in os.walk(root):
    seen.append(rel(dirpath))
    dirnames.clear()
assert seen == ['']
seen = []
for dirpath, dirnames, filenames in os.walk(root):
    seen.append(rel(dirpath))
    if 'deep' in dirnames:
        dirnames.remove('deep')
assert seen == ['', '/subdir']
# names added to `dirnames` may be path-like, as `os.path.join` takes them
seen = []
for dirpath, dirnames, filenames in os.walk(root):
    seen.append(rel(dirpath))
    if 'subdir' in dirnames:
        dirnames.clear()
        dirnames.append(Path('subdir'))
assert seen == ['', '/subdir', '/subdir/deep']

# errors go to `onerror`, and are otherwise ignored
errors = []


def record(error):
    errors.append(error)


assert list(os.walk(root / 'nope', onerror=record)) == []
assert [(type(e).__name__, str(e)) for e in errors] == [
    ('FileNotFoundError', f"[Errno 2] No such file or directory: '{root / 'nope'}'")
]
assert list(os.walk(root / 'nope')) == []
errors.clear()
assert list(os.walk(root / 'hello.txt', onerror=record)) == []
assert [(type(e).__name__, str(e)) for e in errors] == [
    ('NotADirectoryError', f"[Errno 20] Not a directory: '{root / 'hello.txt'}'")
]
errors.clear()
for dirpath, dirnames, filenames in os.walk(str(root), onerror=record):
    if dirpath == str(root):
        dirnames.append('missing')
assert [(type(e).__name__, str(e)) for e in errors] == [
    ('FileNotFoundError', f"[Errno 2] No such file or directory: '{root}/missing'")
]


def fail(error):
    raise RuntimeError('stop')


try:
    list(os.walk(root / 'nope', onerror=fail))
    assert False, 'expected RuntimeError'
except RuntimeError as e:
    assert str(e) == 'stop'
try:
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.append(5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "join() argument must be str, bytes, or os.PathLike object, not 'int'"

# an exception escaping `onerror` ends the walk, as it would end CPython's generator
walk = os.walk(root, onerror=fail)
dirpath, dirnames, filenames = next(walk)
dirnames.clear()
dirnames.extend(['missing', 'subdir'])
try:
    next(walk)
    assert False, 'expected RuntimeError'
except RuntimeError as e:
    assert str(e) == 'stop'
assert list(walk) == []

# an abandoned walk holds its last `dirnames`; one whose `onerror` refers back to it is a cycle
walk = os.walk(root)
dirpath, dirnames, filenames = next(walk)
walk = None
assert dirnames == ['subdir']
holder = []


def hold(error):
    holder.append(error)


holder.append(os.walk(root, onerror=hold))
holder = None

# === os.walk: argument errors ===
try:
    os.walk()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "walk() missing 1 required positional argument: 'top'"
try:
    os.walk(root, foo=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "walk() got an unexpected keyword argument 'foo'"
try:
    os.walk(root, 1, 2, 3, 4)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'walk() takes from 1 to 4 positional arguments but 5 were given'

# === Path.walk ===
path_walked = [(rel(p), sorted(dn), sorted(listed(fn))) for p, dn, fn in root.walk()]
assert path_walked == [(d, sorted(dn), sorted(fn)) for d, dn, fn in walked]
assert all(isinstance(p, Path) for p, _, _ in root.walk())
assert [rel(p) for p, _, _ in root.walk(top_down=False)] == ['/subdir/deep', '/subdir', '']
assert [p.name for p, _, _ in (root / 'subdir').walk()] == ['subdir', 'deep']
errors.clear()
assert list((root / 'nope').walk(on_error=record)) == []
assert [type(e).__name__ for e in errors] == ['FileNotFoundError']
try:
    root.walk(foo=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "Path.walk() got an unexpected keyword argument 'foo'"

# === os.scandir ===
with os.scandir(root) as it:
    entries = {entry.name: entry for entry in it}
assert sorted(entries) == ['data.bin', 'empty.txt', 'hello.txt', 'readonly.txt', 'subdir']
hello = entries['hello.txt']
assert hello.path == str(root / 'hello.txt')
assert hello.is_file()
assert not hello.is_dir()
assert not hello.is_symlink()
assert hello.is_file(follow_symlinks=False)
assert not hello.is_junction()
assert entries['subdir'].is_dir()
assert not entries['subdir'].is_file()
assert repr(hello) == "<DirEntry 'hello.txt'>"
assert type(hello).__name__ == 'DirEntry'
assert {hello: 1}[hello] == 1
assert len({hello, entries['subdir'], hello}) == 2
try:
    (root / 'new.txt').write_text(hello)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'data must be str, not DirEntry'
Point = namedtuple('Point', 'x y')
try:
    (root / 'new.txt').write_text(Point(1, 2))
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'data must be str, not Point'
try:
    (root / 'new.txt').write_text(root)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'data must be str, not PosixPath'
assert hello.stat().st_size == 12
assert os.fspath(hello) == hello.path
assert hello.__fspath__() == hello.path
assert Path(hello) == root / 'hello.txt'
assert open(hello).read() == 'hello world\n'
assert sorted(e.name for e in os.scandir(str(root) + '/subdir')) == ['deep', 'nested.txt']
assert sorted(e.path for e in os.scandir(str(root) + '/subdir/')) == [
    str(root) + '/subdir/deep',
    str(root) + '/subdir/nested.txt',
]

it = os.scandir(root)
assert type(it).__name__ == 'ScandirIterator'
assert next(it).name in entries
it.close()
assert list(it) == []

try:
    os.scandir(root / 'hello.txt')
    assert False, 'expected NotADirectoryError'
except NotADirectoryError as e:
    assert str(e) == f"[Errno 20] Not a directory: '{root / 'hello.txt'}'"
try:
    os.scandir(root / 'nope')
    assert False, 'expected FileNotFoundError'
except FileNotFoundError as e:
    assert str(e) == f"[Errno 2] No such file or directory: '{root / 'nope'}'"
try:
    os.scandir('')
    assert False, 'expected FileNotFoundError'
except FileNotFoundError as e:
    assert str(e) == "[Errno 2] No such file or directory: ''"

# === os.scandir / DirEntry: argument errors ===
try:
    os.scandir(1.5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'scandir: path should be string, bytes, os.PathLike, integer or None, not float'
try:
    os.scandir('a', 'b')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'scandir() takes at most 1 argument (2 given)'
try:
    hello.is_dir(True)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'is_dir() takes no positional arguments'
try:
    hello.is_dir(x=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "is_dir() got an unexpected keyword argument 'x'"
try:
    hello.name = 'x'
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == 'readonly attribute'
try:
    hello.foo
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "'posix.DirEntry' object has no attribute 'foo'"
