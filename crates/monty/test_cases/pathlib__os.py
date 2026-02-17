# call-external
from pathlib import Path

# === exists() ===
assert Path('/virtual/file.txt').exists() == True, 'file exists'
assert Path('/virtual/subdir').exists() == True, 'dir exists'
assert Path('/virtual/subdir/deep').exists() == True, 'nested dir exists'
assert Path('/nonexistent').exists() == False, 'nonexistent path'
assert Path('/nonexistent/file.txt').exists() == False, 'nonexistent nested path'

# === is_file() ===
assert Path('/virtual/file.txt').is_file() == True, 'is_file true for file'
assert Path('/virtual/subdir').is_file() == False, 'is_file false for dir'
assert Path('/nonexistent').is_file() == False, 'is_file false for nonexistent'

# === is_dir() ===
assert Path('/virtual/subdir').is_dir() == True, 'is_dir true for dir'
assert Path('/virtual/file.txt').is_dir() == False, 'is_dir false for file'
assert Path('/nonexistent').is_dir() == False, 'is_dir false for nonexistent'

# === is_symlink() ===
assert Path('/virtual/file.txt').is_symlink() == False, 'is_symlink false (no symlinks in vfs)'
assert Path('/nonexistent').is_symlink() == False, 'is_symlink false for nonexistent'

# === read_text() ===
assert Path('/virtual/file.txt').read_text() == 'hello world\n', 'read_text basic'
assert Path('/virtual/empty.txt').read_text() == '', 'read_text empty file'
assert Path('/virtual/subdir/nested.txt').read_text() == 'nested content', 'read_text nested'
assert Path('/virtual/subdir/deep/file.txt').read_text() == 'deep', 'read_text deep nested'

# === read_bytes() ===
assert Path('/virtual/data.bin').read_bytes() == b'\x00\x01\x02\x03', 'read_bytes binary'
assert Path('/virtual/empty.txt').read_bytes() == b'', 'read_bytes empty'
assert Path('/virtual/file.txt').read_bytes() == b'hello world\n', 'read_bytes text file'

# === stat() basic ===
st = Path('/virtual/file.txt').stat()
assert st.st_size == 12, 'stat size (len of "hello world\\n")'
# 0o644 permissions + regular file type bits (0o100000)
assert st.st_mode & 0o777 == 0o644, 'stat mode permissions'
# Verify it's a regular file using raw mode bits
# S_IFREG = 0o100000, so check that file type bits match
assert st.st_mode & 0o170000 == 0o100000, 'stat is regular file'

# === stat() directory ===
st_dir = Path('/virtual/subdir').stat()
# S_IFDIR = 0o040000, so check that file type bits match
assert st_dir.st_mode & 0o170000 == 0o040000, 'stat is directory'
assert st_dir.st_mode & 0o777 == 0o755, 'stat dir mode permissions'

# === stat() index access ===
st2 = Path('/virtual/file.txt').stat()
assert st2[6] == 12, 'stat index access for st_size'
assert st2[0] & 0o777 == 0o644, 'stat index access for st_mode'

# === iterdir() ===
entries = list(Path('/virtual').iterdir())
assert len(entries) == 5, 'iterdir returns correct count'

# iterdir() should return Path objects, not strings
first_entry = entries[0]
assert isinstance(first_entry, Path), f'iterdir should return Path objects, got {type(first_entry)}'

# Path objects should have .name attribute
names = [e.name for e in entries]
assert 'file.txt' in names, 'iterdir contains file.txt'
assert 'subdir' in names, 'iterdir contains subdir'
assert 'data.bin' in names, 'iterdir contains data.bin'

# Path objects should have .parent attribute
assert entries[0].parent == Path('/virtual'), 'iterdir entry parent is correct'

# === iterdir() nested ===
nested_entries = list(Path('/virtual/subdir').iterdir())
assert len(nested_entries) == 2, 'iterdir nested count'
nested_names = [e.name for e in nested_entries]
assert 'nested.txt' in nested_names, 'iterdir nested contains nested.txt'
assert 'deep' in nested_names, 'iterdir nested contains deep'

# === iterdir() entries can be used for further operations ===
# Find the nested.txt entry and read it
for entry in nested_entries:
    if entry.name == 'nested.txt':
        assert entry.read_text() == 'nested content', 'iterdir entry can be read'

# === resolve() ===
p = Path('/virtual/file.txt').resolve()
assert str(p) == '/virtual/file.txt', 'resolve absolute path unchanged'

# === absolute() ===
p2 = Path('/virtual/subdir').absolute()
assert str(p2) == '/virtual/subdir', 'absolute path unchanged'

# === path concatenation with OS calls ===
base = Path('/virtual')
full = base / 'subdir' / 'nested.txt'
assert full.read_text() == 'nested content', 'path concat then read'
assert full.exists() == True, 'path concat then exists'

# === write_text() ===
Path('/virtual/new_file.txt').write_text('created by write_text')
assert Path('/virtual/new_file.txt').read_text() == 'created by write_text', 'write_text creates file'
# Overwrite existing file
Path('/virtual/file.txt').write_text('overwritten')
assert Path('/virtual/file.txt').read_text() == 'overwritten', 'write_text overwrites'

# === write_bytes() ===
Path('/virtual/binary.dat').write_bytes(b'\xff\xfe\xfd')
assert Path('/virtual/binary.dat').read_bytes() == b'\xff\xfe\xfd', 'write_bytes creates file'

# === mkdir() ===
Path('/virtual/new_dir').mkdir()
assert Path('/virtual/new_dir').is_dir() == True, 'mkdir creates directory'
# mkdir with parents
Path('/virtual/a/b/c').mkdir(parents=True)
assert Path('/virtual/a/b/c').is_dir() == True, 'mkdir parents creates nested'
# mkdir with exist_ok
Path('/virtual/new_dir').mkdir(exist_ok=True)  # Should not raise

# === unlink() ===
Path('/virtual/to_delete.txt').write_text('delete me')
assert Path('/virtual/to_delete.txt').exists() == True, 'file exists before unlink'
Path('/virtual/to_delete.txt').unlink()
assert Path('/virtual/to_delete.txt').exists() == False, 'unlink removes file'

# === rmdir() ===
Path('/virtual/empty_dir').mkdir()
assert Path('/virtual/empty_dir').is_dir() == True, 'dir exists before rmdir'
Path('/virtual/empty_dir').rmdir()
assert Path('/virtual/empty_dir').exists() == False, 'rmdir removes directory'

# === rename() ===
Path('/virtual/old_name.txt').write_text('rename test')
Path('/virtual/old_name.txt').rename(Path('/virtual/new_name.txt'))
assert Path('/virtual/old_name.txt').exists() == False, 'rename removes old path'
assert Path('/virtual/new_name.txt').read_text() == 'rename test', 'rename creates new path'
