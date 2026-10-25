# /// monty
# mount-fs = true
# ///
import sys
import tomllib

is_monty = sys.platform == 'monty'

# root is injected by the test runner: a writable directory on both engines.

# === load() reads a binary file object ===
toml_path = root / 'config.toml'
# written in binary mode so the bytes read back are the same on every platform
with open(toml_path, 'wb') as f:
    f.write(b'[project]\nname = "demo"\nversion = "1.0"\n\n[tool.example]\nenabled = true\nitems = [1, 2, 3]\n')

with open(toml_path, 'rb') as f:
    config = tomllib.load(f)
assert config == {
    'project': {'name': 'demo', 'version': '1.0'},
    'tool': {'example': {'enabled': True, 'items': [1, 2, 3]}},
}

# the whole file is consumed, like fp.read()
with open(toml_path, 'rb') as f:
    tomllib.load(f)
    assert f.read() == b''

# a file already read partway only contributes the remainder
with open(toml_path, 'rb') as f:
    assert f.readline() == b'[project]\n'
    assert tomllib.load(f) == {
        'name': 'demo',
        'version': '1.0',
        'tool': {'example': {'enabled': True, 'items': [1, 2, 3]}},
    }

# === utf-8 content and \r\n newlines ===
with open(root / 'unicode.toml', 'wb') as f:
    f.write('title = "snow ☃"\r\nemoji = "😀"\r\n'.encode())
with open(root / 'unicode.toml', 'rb') as f:
    assert tomllib.load(f) == {'title': 'snow ☃', 'emoji': '😀'}

# === an empty file is an empty document ===
with open(root / 'empty.toml', 'wb') as f:
    pass
with open(root / 'empty.toml', 'rb') as f:
    assert tomllib.load(f) == {}

# === errors ===
with open(root / 'bad.toml', 'wb') as f:
    f.write(b'a = 1\nb =\n')
with open(root / 'bad.toml', 'rb') as f:
    try:
        tomllib.load(f)
        assert False, 'expected TOMLDecodeError'
    except tomllib.TOMLDecodeError as exc:
        assert str(exc) == 'Invalid value (at line 2, column 4)'

with open(root / 'latin1.toml', 'wb') as f:
    f.write(b'a = "\xe9t\xe9"\n')
with open(root / 'latin1.toml', 'rb') as f:
    try:
        tomllib.load(f)
        assert False, 'expected UnicodeDecodeError'
    except UnicodeDecodeError as exc:
        assert str(exc) == "'utf-8' codec can't decode byte 0xe9 in position 5: invalid continuation byte"

# text mode is rejected
with open(toml_path) as f:
    try:
        tomllib.load(f)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == "File must be opened in binary mode, e.g. use `open('foo.toml', 'rb')`"

# a closed file fails as fp.read() would (Monty's binary files report the
# text-mode wording, see limitations/open.md)
f = open(toml_path, 'rb')
f.close()
try:
    tomllib.load(f)
    assert False, 'expected ValueError'
except ValueError as exc:
    if is_monty:
        assert str(exc) == 'I/O operation on closed file.'
    else:
        assert str(exc) == 'read of closed file'

# not a file object at all
try:
    tomllib.load('a = 1')
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'str' object has no attribute 'read'"
try:
    tomllib.load(None)
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'NoneType' object has no attribute 'read'"

# signature
try:
    tomllib.load()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "load() missing 1 required positional argument: 'fp'"

# `parse_float` is not implemented; CPython accepts it
if is_monty:
    with open(toml_path, 'rb') as f:
        try:
            tomllib.load(f, parse_float=float)
            assert False, 'expected TypeError'
        except TypeError as exc:
            assert str(exc) == "load() got an unexpected keyword argument 'parse_float'"
