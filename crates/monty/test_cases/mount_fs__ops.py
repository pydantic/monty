# mount-fs
import os
import sys
from pathlib import Path

# root is injected by the test runner:
# - Monty: Path('/mnt') with OverlayMemory mount over a real temp directory
# - CPython: Path('<real_tmpdir>') pointing to a real temp directory

# === exists() ===
assert (root / 'hello.txt').exists() == True
assert (root / 'subdir').exists() == True
assert (root / 'subdir' / 'deep').exists() == True
assert (root / 'nonexistent').exists() == False
assert (root / 'nonexistent' / 'file.txt').exists() == False

# === is_file() ===
assert (root / 'hello.txt').is_file() == True
assert (root / 'subdir').is_file() == False
assert (root / 'nonexistent').is_file() == False

# === is_dir() ===
assert (root / 'subdir').is_dir() == True
assert (root / 'hello.txt').is_dir() == False
assert (root / 'subdir' / 'deep').is_dir() == True
assert (root / 'nonexistent').is_dir() == False

# === read_text() ===
assert (root / 'hello.txt').read_text() == 'hello world\n'
assert (root / 'empty.txt').read_text() == ''
assert (root / 'subdir' / 'nested.txt').read_text() == 'nested content'
assert (root / 'subdir' / 'deep' / 'file.txt').read_text() == 'deep file'

# === read_bytes() ===
assert (root / 'data.bin').read_bytes() == b'\x00\x01\x02\x03'
assert (root / 'empty.txt').read_bytes() == b''
assert (root / 'hello.txt').read_bytes() == b'hello world\n'

# === write_text() and read back ===
(root / 'new_file.txt').write_text('created by test')
assert (root / 'new_file.txt').read_text() == 'created by test'

# Overwrite existing file
(root / 'hello.txt').write_text('overwritten')
assert (root / 'hello.txt').read_text() == 'overwritten'

# === write_bytes() and read back ===
(root / 'binary.dat').write_bytes(b'\xff\xfe\xfd')
assert (root / 'binary.dat').read_bytes() == b'\xff\xfe\xfd'

# === stat() ===
st = (root / 'readonly.txt').stat()
assert st.st_size == 16

# === iterdir() ===
entries = sorted([e.name for e in root.iterdir()])
assert 'hello.txt' in entries
assert 'subdir' in entries
assert 'data.bin' in entries
assert 'empty.txt' in entries

# iterdir nested
nested_entries = sorted([e.name for e in (root / 'subdir').iterdir()])
assert 'nested.txt' in nested_entries
assert 'deep' in nested_entries

# iterdir entries can be used for further operations
for entry in (root / 'subdir').iterdir():
    if entry.name == 'nested.txt':
        assert entry.read_text() == 'nested content'

# === mkdir() ===
(root / 'new_dir').mkdir()
assert (root / 'new_dir').is_dir() == True

# mkdir with parents
(root / 'a' / 'b' / 'c').mkdir(parents=True)
assert (root / 'a' / 'b' / 'c').is_dir() == True

# mkdir with positional mode and parents
(root / 'positional_parent' / 'child').mkdir(0o777, True)
assert (root / 'positional_parent' / 'child').is_dir() == True

# mkdir with exist_ok on existing directory
(root / 'new_dir').mkdir(exist_ok=True)

# mkdir(parents=True) on fresh nested path
(root / 'd' / 'e' / 'f').mkdir(parents=True)
assert (root / 'd' / 'e' / 'f').is_dir() == True

# mkdir(parents=True, exist_ok=True) on existing directory
(root / 'new_dir').mkdir(parents=True, exist_ok=True)
assert (root / 'new_dir').is_dir() == True

# mkdir(parents=True, exist_ok=True) on existing nested directory
(root / 'a' / 'b' / 'c').mkdir(parents=True, exist_ok=True)
assert (root / 'a' / 'b' / 'c').is_dir() == True

# mkdir(parents=True) where some parents already exist
(root / 'a' / 'b' / 'new_child').mkdir(parents=True)
assert (root / 'a' / 'b' / 'new_child').is_dir() == True

# === unlink() ===
(root / 'to_delete.txt').write_text('delete me')
assert (root / 'to_delete.txt').exists() == True
(root / 'to_delete.txt').unlink()
assert (root / 'to_delete.txt').exists() == False

# === rmdir() ===
(root / 'empty_dir').mkdir()
assert (root / 'empty_dir').is_dir() == True
(root / 'empty_dir').rmdir()
assert (root / 'empty_dir').exists() == False

# === rename() ===
(root / 'old_name.txt').write_text('rename test')
(root / 'old_name.txt').rename(root / 'new_name.txt')
assert (root / 'old_name.txt').exists() == False
assert (root / 'new_name.txt').read_text() == 'rename test'

# === write_text() return value is character count, not byte count ===
n = (root / 'unicode.txt').write_text('hello')
assert n == 5, f'write_text ASCII returns char count: {n}'

if sys.platform != 'win32':
    n = (root / 'unicode.txt').write_text('\U0001f600')  # single emoji = 1 char, 4 UTF-8 bytes
    assert n == 1, f'write_text emoji returns char count not byte count: {n}'
    n = (root / 'unicode.txt').write_text('\u00e9')  # é = 1 char, 2 UTF-8 bytes
    assert n == 1, f'write_text accented char returns char count: {n}'
    n = (root / 'unicode.txt').write_text('\u4e16\u754c')  # 世界 = 2 chars, 6 UTF-8 bytes
    assert n == 2, f'write_text CJK returns char count: {n}'

# === mkdir(parents=True) over deleted parent ===
(root / 'mkp_test').mkdir()
(root / 'mkp_test' / 'child.txt').write_text('data')
(root / 'mkp_test' / 'child.txt').unlink()
(root / 'mkp_test').rmdir()
assert not (root / 'mkp_test').exists(), 'deleted dir should not exist'
# Re-create with parents=True over the tombstoned path
(root / 'mkp_test' / 'sub' / 'deep').mkdir(parents=True)
assert (root / 'mkp_test' / 'sub' / 'deep').is_dir()

# === mkdir(parents=True) blocked by real file ===
try:
    (root / 'hello.txt' / 'sub').mkdir(parents=True)
    assert False, 'expected error when mkdir parents through a file'
except (OSError, NotADirectoryError):
    pass  # correct — a file blocks mkdir -p

# === resolve() and absolute() ===
p = (root / 'hello.txt').resolve()
assert p.name == 'hello.txt'

p2 = (root / 'subdir').absolute()
assert p2.name == 'subdir'

# === path operations with mounted paths ===
full = root / 'subdir' / 'nested.txt'
assert full.name == 'nested.txt'
assert full.suffix == '.txt'
assert full.stem == 'nested'

# === os module wrappers (route through the same OS calls as pathlib) ===

# os.mkdir / os.listdir
os.mkdir(root / 'os_dir')
assert (root / 'os_dir').is_dir() == True
(root / 'os_dir' / 'one.txt').write_text('1')
(root / 'os_dir' / 'two.txt').write_text('2')
os.mkdir(root / 'os_dir' / 'sub')
assert sorted(os.listdir(root / 'os_dir')) == ['one.txt', 'sub', 'two.txt']

# str paths work too, and entries are plain strings (names, not paths)
listing = os.listdir(str(root / 'os_dir'))
assert type(listing) is list
for entry in listing:
    assert type(entry) is str
assert sorted(listing) == ['one.txt', 'sub', 'two.txt']

# empty directory
assert os.listdir(root / 'os_dir' / 'sub') == []

# os.listdir errors surface the host exception. Message text is asserted
# only off-Windows: Windows CPython uses WinError wording ([WinError 3] /
# [WinError 267] / ...), while Monty keeps the POSIX messages on every host.
posix_messages = sys.platform != 'win32'
try:
    os.listdir(root / 'os_dir' / 'missing')
    assert False, 'expected FileNotFoundError'
except FileNotFoundError as e:
    if posix_messages:
        assert str(e) == f"[Errno 2] No such file or directory: '{root / 'os_dir' / 'missing'}'"
try:
    os.listdir(root / 'os_dir' / 'one.txt')
    assert False, 'expected NotADirectoryError'
except NotADirectoryError as e:
    if posix_messages:
        assert str(e) == f"[Errno 20] Not a directory: '{root / 'os_dir' / 'one.txt'}'"

# os.stat — same stat_result as Path.stat(), defaulted kwargs accepted
st = os.stat(root / 'readonly.txt')
assert st.st_size == 16
st = os.stat(root / 'readonly.txt', dir_fd=None, follow_symlinks=True)
assert st.st_size == 16
try:
    os.stat(root / 'os_dir' / 'missing')
    assert False, 'expected FileNotFoundError'
except FileNotFoundError as e:
    if posix_messages:
        assert str(e) == f"[Errno 2] No such file or directory: '{root / 'os_dir' / 'missing'}'"

# os.mkdir on an existing directory
try:
    os.mkdir(root / 'os_dir')
    assert False, 'expected FileExistsError'
except FileExistsError as e:
    if posix_messages:
        assert str(e) == f"[Errno 17] File exists: '{root / 'os_dir'}'"

# os.makedirs
os.makedirs(root / 'os_deep' / 'x' / 'y')
assert (root / 'os_deep' / 'x' / 'y').is_dir() == True
os.makedirs(root / 'os_deep' / 'x' / 'y', exist_ok=True)
try:
    os.makedirs(root / 'os_deep' / 'x' / 'y')
    assert False, 'expected FileExistsError'
except FileExistsError as e:
    if posix_messages:
        assert str(e) == f"[Errno 17] File exists: '{root / 'os_deep' / 'x' / 'y'}'"

# os.remove / os.unlink
os.remove(root / 'os_dir' / 'one.txt')
assert (root / 'os_dir' / 'one.txt').exists() == False
os.unlink(root / 'os_dir' / 'two.txt')
assert (root / 'os_dir' / 'two.txt').exists() == False
try:
    os.remove(root / 'os_dir' / 'one.txt')
    assert False, 'expected FileNotFoundError'
except FileNotFoundError as e:
    if posix_messages:
        assert str(e) == f"[Errno 2] No such file or directory: '{root / 'os_dir' / 'one.txt'}'"

# os.rmdir
os.rmdir(root / 'os_dir' / 'sub')
assert (root / 'os_dir' / 'sub').exists() == False

# os.rename / os.replace
(root / 'os_move_a.txt').write_text('move me')
os.rename(root / 'os_move_a.txt', root / 'os_move_b.txt')
assert (root / 'os_move_a.txt').exists() == False
assert (root / 'os_move_b.txt').read_text() == 'move me'
os.replace(root / 'os_move_b.txt', root / 'os_move_c.txt')
assert (root / 'os_move_b.txt').exists() == False
assert (root / 'os_move_c.txt').read_text() == 'move me'
