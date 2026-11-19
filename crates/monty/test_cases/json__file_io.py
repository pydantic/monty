# /// monty
# mount-fs = true
# skip-cpython-windows = true
# ///
# CPython on Windows defaults text I/O to cp1252, which can't encode é.
import json
import sys

is_monty = sys.platform == 'monty'

# === dump then load round trip ===
path = root / 'data.json'
with open(path, 'w') as f:
    assert json.dump({'a': 1, 'b': [True, None, 'x'], 'c': {'d': 1.5}}, f) is None
assert path.read_text() == '{"a": 1, "b": [true, null, "x"], "c": {"d": 1.5}}'

with open(path) as f:
    assert json.load(f) == {'a': 1, 'b': [True, None, 'x'], 'c': {'d': 1.5}}

# === load reads to EOF, so a second load sees nothing ===
with open(path) as f:
    json.load(f)
    assert f.read() == ''
    try:
        json.load(f)
        assert False, 'expected a second load to fail'
    except json.JSONDecodeError as exc:
        assert str(exc) == 'Expecting value: line 1 column 1 (char 0)'

# === load parses the rest of a partially read file ===
with open(path) as f:
    assert f.read(2) == '{"'
    try:
        json.load(f)
        assert False, 'expected load after a partial read to fail'
    except json.JSONDecodeError as exc:
        assert str(exc) == 'Expecting value: line 1 column 1 (char 0)'

path.write_text('[1, 2, 3]')
with open(path) as f:
    assert f.read(4) == '[1, '
    f.seek(0)
    assert json.load(f) == [1, 2, 3]
    assert f.tell() == 9

# === load from a binary file ===
with open(path, 'rb') as f:
    assert json.load(f) == [1, 2, 3]

# a binary file may start with a BOM; its text does too, which `load` rejects
path.write_bytes(b'\xef\xbb\xbf{"bom": 1}')
with open(path, 'rb') as f:
    assert json.load(f) == {'bom': 1}
with open(path, encoding='utf-8') as f:
    try:
        json.load(f)
        assert False, 'expected a BOM in text to fail'
    except json.JSONDecodeError as exc:
        assert str(exc) == 'Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)'

# === load through iteration state ===
path.write_text('{"k": "v"}\n')
with open(path) as f:
    assert f.readline() == '{"k": "v"}\n'
    try:
        json.load(f)
        assert False, 'expected load at EOF to fail'
    except json.JSONDecodeError as exc:
        assert str(exc) == 'Expecting value: line 1 column 1 (char 0)'

# === load errors name the position within what was read ===
path.write_text('{"a": 1} x')
with open(path) as f:
    try:
        json.load(f)
        assert False, 'expected trailing data to fail'
    except json.JSONDecodeError as exc:
        assert str(exc) == 'Extra data: line 1 column 10 (char 9)'
    # the failed parse still consumed the file
    assert f.read() == ''

with open(root / 'empty.txt') as f:
    try:
        json.load(f)
        assert False, 'expected an empty file to fail'
    except json.JSONDecodeError as exc:
        assert str(exc) == 'Expecting value: line 1 column 1 (char 0)'

# === dump keyword arguments ===
with open(path, 'w') as f:
    json.dump({'b': [1, 2], 'a': 'é'}, f, indent=2, sort_keys=True)
assert path.read_text() == '{\n  "a": "\\u00e9",\n  "b": [\n    1,\n    2\n  ]\n}'

with open(path, 'w') as f:
    json.dump({'b': [1, 2], 'a': 'é'}, f, ensure_ascii=False, separators=(',', ':'))
assert path.read_bytes() == b'{"b":[1,2],"a":"\xc3\xa9"}'

with open(path, 'w') as f:
    json.dump({(1, 2): 1, 'ok': 2}, f, skipkeys=True)
assert path.read_text() == '{"ok": 2}'

with open(path, 'w') as f:
    try:
        json.dump(float('nan'), f, allow_nan=False)
        assert False, 'expected nan to be rejected'
    except ValueError as exc:
        assert str(exc) == 'Out of range float values are not JSON compliant: nan'
assert path.read_text() == ''

# === dump appends to earlier writes and advances the position ===
with open(path, 'w') as f:
    assert f.write('x') == 1
    json.dump([1], f)
    assert f.tell() == 4
    json.dump([2], f)
assert path.read_text() == 'x[1][2]'

with open(path, 'a') as f:
    json.dump([3], f)
assert path.read_text() == 'x[1][2][3]'

# === dump builds the encoder even for a str document ===
with open(path, 'w') as f:
    try:
        json.dump('x', f, indent=0.5)
        assert False, 'float indent should raise TypeError'
    except TypeError as exc:
        assert str(exc) == "can't multiply sequence by non-int of type 'float'"

# === dump honours the file's newline translation ===
with open(path, 'w', newline='\r\n') as f:
    json.dump([1], f, indent=1)
assert path.read_bytes() == b'[\r\n 1\r\n]'

# === dump to a Path.open() handle ===
with path.open('w') as f:
    json.dump('text', f)
with path.open() as f:
    assert json.load(f) == 'text'

# === dump into a binary, read-only or closed file ===
with open(path, 'wb') as f:
    try:
        json.dump({'a': 1}, f)
        assert False, 'expected dump to a binary file to fail'
    except TypeError as exc:
        assert str(exc) == "a bytes-like object is required, not 'str'"
assert path.read_bytes() == b''

with open(path) as f:
    try:
        json.dump({'a': 1}, f)
        assert False, 'expected dump to a read-only file to fail'
    except ValueError as exc:
        assert str(exc) == 'not writable'
    except OSError as exc:
        assert False, f'io.UnsupportedOperation is a ValueError first: {exc}'

f = open(path, 'w')
f.close()
try:
    json.dump({'a': 1}, f)
    assert False, 'expected dump to a closed file to fail'
except ValueError as exc:
    assert str(exc) == 'I/O operation on closed file.'
try:
    json.load(f)
    assert False, 'expected load from a closed file to fail'
except ValueError as exc:
    assert str(exc) == 'I/O operation on closed file.'

# === load from a write-only file ===
with open(path, 'w') as f:
    try:
        json.load(f)
        assert False, 'expected load from a write-only file to fail'
    except ValueError as exc:
        assert str(exc) == 'not readable'

# === the file object is checked before a container's contents are encoded ===
with open(path) as f:
    try:
        json.dump({'a': {1}}, f)
        assert False, 'expected dump to a read-only file to fail'
    except ValueError as exc:
        assert str(exc) == 'not writable'

# an unencodable scalar is reported before the file object is touched
with open(path) as f:
    try:
        json.dump({1}, f)
        assert False, 'expected a set to be rejected'
    except TypeError as exc:
        assert str(exc) == 'Object of type set is not JSON serializable'

# === objects without the file method ===
for bad, name in [(1, 'int'), (None, 'NoneType'), ('{"a": 1}', 'str'), ([], 'list')]:
    try:
        json.load(bad)
        assert False, 'expected load without read() to fail'
    except AttributeError as exc:
        assert str(exc) == f"'{name}' object has no attribute 'read'"
    try:
        json.dump([1], bad)
        assert False, 'expected dump without write() to fail'
    except AttributeError as exc:
        assert str(exc) == f"'{name}' object has no attribute 'write'"


# === duck-typed file objects ===
class Reader:
    def __init__(self, text):
        self.text = text
        self.calls = 0

    def read(self):
        self.calls += 1
        return self.text


reader = Reader('{"z": [1]}')
assert json.load(reader) == {'z': [1]}
assert reader.calls == 1
assert json.load(Reader(b'[1, 2]')) == [1, 2]
try:
    json.load(Reader(5))
    assert False, 'expected a non-text read() result to fail'
except TypeError as exc:
    assert str(exc) == 'the JSON object must be str, bytes or bytearray, not int'


class BadReader:
    read = 5


try:
    json.load(BadReader())
    assert False, 'expected a non-callable read to fail'
except TypeError as exc:
    assert str(exc) == "'int' object is not callable"


class BadWriter:
    write = 5


# a container's `[` reaches CPython's `fp.write` before its bad element fails to encode
for obj in [[{1}], {'a': {1}}, 1]:
    try:
        json.dump(obj, BadWriter())
        assert False, 'expected a non-callable write to fail'
    except TypeError as exc:
        assert str(exc) == "'int' object is not callable"


class Writer:
    def __init__(self):
        self.chunks = []

    def write(self, chunk):
        self.chunks.append(chunk)
        return 'ignored'


# a `str` document never reaches the pure-Python encoder's separators
writer = Writer()
json.dump('x', writer, separators=[1, 2])
assert writer.chunks == ['"x"']

writer = Writer()
assert json.dump({'a': [1, 2], 'b': 'x'}, writer) is None
# CPython streams the document in pieces; Monty writes it in one call
assert ''.join(writer.chunks) == '{"a": [1, 2], "b": "x"}'
if is_monty:
    assert writer.chunks == ['{"a": [1, 2], "b": "x"}']


class FailingWriter:
    def write(self, chunk):
        raise ValueError('boom')


try:
    json.dump([1], FailingWriter())
    assert False, 'expected the write error to propagate'
except ValueError as exc:
    assert str(exc) == 'boom'

# === keyword forms of the positionals ===
with open(path, 'w') as f:
    assert json.dump(obj=[1], fp=f) is None
with open(path) as f:
    assert json.load(fp=f) == [1]
