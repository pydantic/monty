# mount-fs
import sys
from pathlib import Path

# root is injected by the test runner:
# - Monty: Path('/mnt') with OverlayMemory mount over a real temp directory
# - CPython: Path('<real_tmpdir>') pointing to a real temp directory

# === exists() ===
assert (root / 'hello.txt').exists() == True, 'file exists'
assert (root / 'subdir').exists() == True, 'dir exists'
assert (root / 'subdir' / 'deep').exists() == True, 'nested dir exists'
assert (root / 'nonexistent').exists() == False, 'nonexistent path'
assert (root / 'nonexistent' / 'file.txt').exists() == False, 'nonexistent nested'

# === is_file() ===
assert (root / 'hello.txt').is_file() == True, 'is_file on file'
assert (root / 'subdir').is_file() == False, 'is_file on dir'
assert (root / 'nonexistent').is_file() == False, 'is_file on nonexistent'

# === is_dir() ===
assert (root / 'subdir').is_dir() == True, 'is_dir on dir'
assert (root / 'hello.txt').is_dir() == False, 'is_dir on file'
assert (root / 'subdir' / 'deep').is_dir() == True, 'is_dir nested'
assert (root / 'nonexistent').is_dir() == False, 'is_dir on nonexistent'

# === read_text() ===
assert (root / 'hello.txt').read_text() == 'hello world\n', 'read_text basic'
assert (root / 'empty.txt').read_text() == '', 'read_text empty'
assert (root / 'subdir' / 'nested.txt').read_text() == 'nested content', 'read_text nested'
assert (root / 'subdir' / 'deep' / 'file.txt').read_text() == 'deep file', 'read_text deep'

# === read_bytes() ===
assert (root / 'data.bin').read_bytes() == b'\x00\x01\x02\x03', 'read_bytes binary'
assert (root / 'empty.txt').read_bytes() == b'', 'read_bytes empty'
assert (root / 'hello.txt').read_bytes() == b'hello world\n', 'read_bytes text file'

# === write_text() and read back ===
(root / 'new_file.txt').write_text('created by test')
assert (root / 'new_file.txt').read_text() == 'created by test', 'write_text creates file'

# Overwrite existing file
(root / 'hello.txt').write_text('overwritten')
assert (root / 'hello.txt').read_text() == 'overwritten', 'write_text overwrites'

# === write_bytes() and read back ===
(root / 'binary.dat').write_bytes(b'\xff\xfe\xfd')
assert (root / 'binary.dat').read_bytes() == b'\xff\xfe\xfd', 'write_bytes creates file'

# === stat() ===
st = (root / 'readonly.txt').stat()
assert st.st_size == 16, 'stat size (len of "readonly content")'

# === iterdir() ===
entries = sorted([e.name for e in root.iterdir()])
assert 'hello.txt' in entries, 'iterdir has hello.txt'
assert 'subdir' in entries, 'iterdir has subdir'
assert 'data.bin' in entries, 'iterdir has data.bin'
assert 'empty.txt' in entries, 'iterdir has empty.txt'

# iterdir nested
nested_entries = sorted([e.name for e in (root / 'subdir').iterdir()])
assert 'nested.txt' in nested_entries, 'iterdir nested has nested.txt'
assert 'deep' in nested_entries, 'iterdir nested has deep'

# iterdir entries can be used for further operations
for entry in (root / 'subdir').iterdir():
    if entry.name == 'nested.txt':
        assert entry.read_text() == 'nested content', 'iterdir entry readable'

# === mkdir() ===
(root / 'new_dir').mkdir()
assert (root / 'new_dir').is_dir() == True, 'mkdir creates dir'

# mkdir with parents
(root / 'a' / 'b' / 'c').mkdir(parents=True)
assert (root / 'a' / 'b' / 'c').is_dir() == True, 'mkdir parents'

# mkdir with exist_ok
(root / 'new_dir').mkdir(exist_ok=True)

# === unlink() ===
(root / 'to_delete.txt').write_text('delete me')
assert (root / 'to_delete.txt').exists() == True, 'file before unlink'
(root / 'to_delete.txt').unlink()
assert (root / 'to_delete.txt').exists() == False, 'file after unlink'

# === rmdir() ===
(root / 'empty_dir').mkdir()
assert (root / 'empty_dir').is_dir() == True, 'dir before rmdir'
(root / 'empty_dir').rmdir()
assert (root / 'empty_dir').exists() == False, 'dir after rmdir'

# === rename() ===
(root / 'old_name.txt').write_text('rename test')
(root / 'old_name.txt').rename(root / 'new_name.txt')
assert (root / 'old_name.txt').exists() == False, 'old name gone after rename'
assert (root / 'new_name.txt').read_text() == 'rename test', 'new name readable'

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
assert (root / 'mkp_test' / 'sub' / 'deep').is_dir(), 'mkdir parents recreates over tombstone'

# === mkdir(parents=True) blocked by real file ===
try:
    (root / 'hello.txt' / 'sub').mkdir(parents=True)
    assert False, 'expected error when mkdir parents through a file'
except (OSError, NotADirectoryError):
    pass  # correct — a file blocks mkdir -p

# === resolve() and absolute() ===
p = (root / 'hello.txt').resolve()
assert p.name == 'hello.txt', 'resolve preserves name'

p2 = (root / 'subdir').absolute()
assert p2.name == 'subdir', 'absolute preserves name'

# === path operations with mounted paths ===
full = root / 'subdir' / 'nested.txt'
assert full.name == 'nested.txt', 'path / .name'
assert full.suffix == '.txt', 'path / .suffix'
assert full.stem == 'nested', 'path / .stem'
