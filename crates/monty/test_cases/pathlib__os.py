# call-external
# skip-cpython-windows — pathlib uses POSIX paths in Monty's sandbox, Windows CPython resolves differently
from pathlib import Path

# === exists() ===
assert Path('/virtual/file.txt').exists() == True
assert Path('/virtual/subdir').exists() == True
assert Path('/virtual/subdir/deep').exists() == True
assert Path('/nonexistent').exists() == False
assert Path('/nonexistent/file.txt').exists() == False

# === is_file() ===
assert Path('/virtual/file.txt').is_file() == True
assert Path('/virtual/subdir').is_file() == False
assert Path('/nonexistent').is_file() == False

# === is_dir() ===
assert Path('/virtual/subdir').is_dir() == True
assert Path('/virtual/file.txt').is_dir() == False
assert Path('/nonexistent').is_dir() == False

# === is_symlink() ===
assert Path('/virtual/file.txt').is_symlink() == False
assert Path('/nonexistent').is_symlink() == False

# === read_text() ===
assert Path('/virtual/file.txt').read_text() == 'hello world\n'
assert Path('/virtual/empty.txt').read_text() == ''
assert Path('/virtual/subdir/nested.txt').read_text() == 'nested content'
assert Path('/virtual/subdir/deep/file.txt').read_text() == 'deep'

# === read_bytes() ===
assert Path('/virtual/data.bin').read_bytes() == b'\x00\x01\x02\x03'
assert Path('/virtual/empty.txt').read_bytes() == b''
assert Path('/virtual/file.txt').read_bytes() == b'hello world\n'

# === stat() basic ===
st = Path('/virtual/file.txt').stat()
assert st.st_size == 12
# 0o644 permissions + regular file type bits (0o100000)
assert st.st_mode & 0o777 == 0o644
# Verify it's a regular file using raw mode bits
# S_IFREG = 0o100000, so check that file type bits match
assert st.st_mode & 0o170000 == 0o100000

# === stat() directory ===
st_dir = Path('/virtual/subdir').stat()
# S_IFDIR = 0o040000, so check that file type bits match
assert st_dir.st_mode & 0o170000 == 0o040000
assert st_dir.st_mode & 0o777 == 0o755

# === stat() index access ===
st2 = Path('/virtual/file.txt').stat()
assert st2[6] == 12
assert st2[0] & 0o777 == 0o644

# === iterdir() ===
entries = list(Path('/virtual').iterdir())
assert len(entries) == 5

# iterdir() should return Path objects, not strings
first_entry = entries[0]
assert isinstance(first_entry, Path), f'iterdir should return Path objects, got {type(first_entry)}'

# Path objects should have .name attribute
names = [e.name for e in entries]
assert 'file.txt' in names
assert 'subdir' in names
assert 'data.bin' in names

# Path objects should have .parent attribute
assert entries[0].parent == Path('/virtual')

# === iterdir() nested ===
nested_entries = list(Path('/virtual/subdir').iterdir())
assert len(nested_entries) == 2
nested_names = [e.name for e in nested_entries]
assert 'nested.txt' in nested_names
assert 'deep' in nested_names

# === iterdir() entries can be used for further operations ===
# Find the nested.txt entry and read it
for entry in nested_entries:
    if entry.name == 'nested.txt':
        assert entry.read_text() == 'nested content'

# === resolve() ===
p = Path('/virtual/file.txt').resolve()
assert str(p) == '/virtual/file.txt'

# === absolute() ===
p2 = Path('/virtual/subdir').absolute()
assert str(p2) == '/virtual/subdir'

# === path concatenation with OS calls ===
base = Path('/virtual')
full = base / 'subdir' / 'nested.txt'
assert full.read_text() == 'nested content'
assert full.exists() == True

# === write_text() ===
Path('/virtual/new_file.txt').write_text('created by write_text')
assert Path('/virtual/new_file.txt').read_text() == 'created by write_text'
# Overwrite existing file
Path('/virtual/file.txt').write_text('overwritten')
assert Path('/virtual/file.txt').read_text() == 'overwritten'

# === write_bytes() ===
Path('/virtual/binary.dat').write_bytes(b'\xff\xfe\xfd')
assert Path('/virtual/binary.dat').read_bytes() == b'\xff\xfe\xfd'

# === mkdir() ===
Path('/virtual/new_dir').mkdir()
assert Path('/virtual/new_dir').is_dir() == True
# mkdir with parents
Path('/virtual/a/b/c').mkdir(parents=True)
assert Path('/virtual/a/b/c').is_dir() == True
# mkdir with exist_ok
Path('/virtual/new_dir').mkdir(exist_ok=True)  # Should not raise
# Empty containers/strings/bytes are falsy in Python — exist_ok='' must NOT
# suppress FileExistsError, and parents='' must NOT create missing parents.
try:
    Path('/virtual/new_dir').mkdir(exist_ok='')
    assert False, "mkdir(exist_ok='') should raise (empty string is falsy)"
except OSError as exc:
    assert isinstance(exc, FileExistsError), f'expected FileExistsError, got {type(exc).__name__}'
try:
    Path('/virtual/x/y/z').mkdir(parents=b'')
    assert False, "mkdir(parents=b'') should raise when parents missing"
except OSError as exc:
    assert isinstance(exc, FileNotFoundError), f'expected FileNotFoundError, got {type(exc).__name__}'
try:
    Path('/virtual/x/y/z').mkdir(parents=[])
    assert False, 'mkdir(parents=[]) should raise when parents missing'
except OSError as exc:
    assert isinstance(exc, FileNotFoundError), f'expected FileNotFoundError, got {type(exc).__name__}'
# Non-empty containers/strings ARE truthy.
Path('/virtual/p/q/r').mkdir(parents='yes')
assert Path('/virtual/p/q/r').is_dir() == True
Path('/virtual/new_dir').mkdir(exist_ok=[0])  # truthy list, should not raise

# === unlink() ===
Path('/virtual/to_delete.txt').write_text('delete me')
assert Path('/virtual/to_delete.txt').exists() == True
Path('/virtual/to_delete.txt').unlink()
assert Path('/virtual/to_delete.txt').exists() == False

# === rmdir() ===
Path('/virtual/empty_dir').mkdir()
assert Path('/virtual/empty_dir').is_dir() == True
Path('/virtual/empty_dir').rmdir()
assert Path('/virtual/empty_dir').exists() == False

# === rename() ===
Path('/virtual/old_name.txt').write_text('rename test')
Path('/virtual/old_name.txt').rename(Path('/virtual/new_name.txt'))
assert Path('/virtual/old_name.txt').exists() == False
assert Path('/virtual/new_name.txt').read_text() == 'rename test'
