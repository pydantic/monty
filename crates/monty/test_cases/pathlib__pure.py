# === Path constructor ===
from pathlib import Path

p = Path('/usr/local/bin/python')
assert str(p) == '/usr/local/bin/python', 'Path str should match input'

# Constructor with multiple arguments
assert str(Path('folder', 'file.txt')) == 'folder/file.txt', 'Path with two args joins'
assert str(Path('/usr', 'local', 'bin')) == '/usr/local/bin', 'Path with three args joins'
assert str(Path('start', '/absolute', 'end')) == '/absolute/end', 'absolute in middle replaces'

# Constructor with no arguments
assert str(Path()) == '.', 'Path() returns current dir'

# === name property ===
assert p.name == 'python', 'name should be final component'
assert Path('/usr/local/bin/').name == 'bin', 'name should handle trailing slash'
assert Path('/').name == '', 'root path should have empty name'
assert Path('file.txt').name == 'file.txt', 'relative path name'

# === parent property ===
assert str(p.parent) == '/usr/local/bin', 'parent should remove last component'
assert str(Path('/usr').parent) == '/', 'parent of first-level should be root'
assert str(Path('/').parent) == '/', 'parent of root is root'
assert str(Path('file.txt').parent) == '.', 'parent of relative without dir is .'

# === stem property ===
assert Path('/path/file.tar.gz').stem == 'file.tar', 'stem removes last extension'
assert Path('/path/file.txt').stem == 'file', 'stem removes single extension'
assert Path('/path/.bashrc').stem == '.bashrc', 'stem preserves hidden files'
assert Path('/path/file').stem == 'file', 'stem without extension'

# === suffix property ===
assert Path('/path/file.tar.gz').suffix == '.gz', 'suffix is last extension'
assert Path('/path/file.txt').suffix == '.txt', 'suffix with single extension'
assert Path('/path/.bashrc').suffix == '', 'hidden file has no suffix'
assert Path('/path/file').suffix == '', 'no extension means empty suffix'

# === suffixes property ===
assert Path('/path/file.tar.gz').suffixes == ['.tar', '.gz'], 'suffixes list'
assert Path('/path/file.txt').suffixes == ['.txt'], 'single suffix as list'
assert Path('/path/.bashrc').suffixes == [], 'hidden file has no suffixes'

# === parts property ===
assert Path('/usr/local/bin').parts == ('/', 'usr', 'local', 'bin'), 'absolute path parts'
assert Path('usr/local').parts == ('usr', 'local'), 'relative path parts'
assert Path('/').parts == ('/',), 'root path parts'

# === is_absolute method ===
assert Path('/usr/bin').is_absolute() == True, 'absolute path'
assert Path('usr/bin').is_absolute() == False, 'relative path not absolute'
assert Path('').is_absolute() == False, 'empty path not absolute'

# === joinpath method ===
assert str(Path('/usr').joinpath('local')) == '/usr/local', 'joinpath with one arg'
assert str(Path('/usr').joinpath('local', 'bin')) == '/usr/local/bin', 'joinpath with two args'
assert str(Path('/usr').joinpath('/etc')) == '/etc', 'joinpath with absolute replaces'
assert str(Path('.').joinpath('file')) == 'file', 'joinpath from dot'

# === with_name method ===
assert str(Path('/path/file.txt').with_name('other.py')) == '/path/other.py', 'with_name replaces name'
assert str(Path('file.txt').with_name('other.py')) == 'other.py', 'with_name on relative'

# === with_suffix method ===
assert str(Path('/path/file.txt').with_suffix('.py')) == '/path/file.py', 'with_suffix replaces'
assert str(Path('/path/file.txt').with_suffix('')) == '/path/file', 'with_suffix removes'
assert str(Path('/path/file').with_suffix('.txt')) == '/path/file.txt', 'with_suffix adds'

# === / operator ===
assert str(Path('/usr') / 'local') == '/usr/local', '/ operator joins'
assert str(Path('/usr') / 'local' / 'bin') == '/usr/local/bin', '/ operator chains'

# === as_posix method ===
assert Path('/usr/bin').as_posix() == '/usr/bin', 'as_posix returns string'

# === __fspath__ method (os.PathLike protocol) ===
assert Path('/usr/bin').__fspath__() == '/usr/bin', '__fspath__ returns string'

# === repr ===
r = repr(Path('/usr/bin'))
assert r == "PosixPath('/usr/bin')", f'repr should be PosixPath, got {r}'
