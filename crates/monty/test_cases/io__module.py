# /// monty
# mount-fs = true
# ///
import csv
import io
import json
from io import BytesIO, StringIO


def check(exc_type, message, fn):
    try:
        fn()
    except exc_type as e:
        assert type(e) is exc_type
        assert str(e) == message
    else:
        assert False, 'did not raise'


# === constants ===
assert io.SEEK_SET == 0
assert io.SEEK_CUR == 1
assert io.SEEK_END == 2
assert io.DEFAULT_BUFFER_SIZE == 131072

# === exceptions ===
assert io.UnsupportedOperation.__name__ == 'UnsupportedOperation'
assert issubclass(io.UnsupportedOperation, OSError)
assert issubclass(io.UnsupportedOperation, ValueError)
assert io.BlockingIOError is BlockingIOError
assert issubclass(BlockingIOError, OSError)
assert BlockingIOError.__mro__ == (BlockingIOError, OSError, Exception, BaseException, object)
assert str(BlockingIOError('would block')) == 'would block'
try:
    raise io.BlockingIOError('again')
except OSError as e:
    assert type(e) is BlockingIOError
    assert e.args == ('again',)

# === the file classes `open()` returns ===
assert io.open is open
with open(root / 'io_text.txt', 'w', newline='') as f:
    assert type(f) is io.TextIOWrapper
    assert isinstance(f, io.TextIOWrapper)
    f.write('one\ntwo\n')
with open(root / 'io_text.txt', 'rb') as f:
    assert type(f) is io.BufferedReader
    assert f.read() == b'one\ntwo\n'
with open(root / 'io_bin.bin', 'wb') as f:
    assert type(f) is io.BufferedWriter
with open(root / 'io_bin.bin', 'w+b') as f:
    assert type(f) is io.BufferedRandom
assert repr(io.TextIOWrapper) == "<class '_io.TextIOWrapper'>"
assert repr(io.BufferedReader) == "<class '_io.BufferedReader'>"
assert repr(io.BufferedWriter) == "<class '_io.BufferedWriter'>"
assert repr(io.BufferedRandom) == "<class '_io.BufferedRandom'>"
assert not isinstance(StringIO(), io.TextIOWrapper)

# === open_code ===
with io.open_code(str(root / 'io_text.txt')) as f:
    assert type(f) is io.BufferedReader
    assert f.read() == b'one\ntwo\n'
check(TypeError, "open_code() argument 'path' must be str, not int", lambda: io.open_code(1))
check(TypeError, "open_code() missing required argument 'path' (pos 1)", lambda: io.open_code())

# === text_encoding ===
assert io.text_encoding(None) == 'locale'
assert io.text_encoding('latin-1') == 'latin-1'
assert io.text_encoding(None, 3) == 'locale'
assert io.text_encoding(1) == 1
check(TypeError, 'text_encoding expected at least 1 argument, got 0', lambda: io.text_encoding())
check(TypeError, '_io.text_encoding() takes no keyword arguments', lambda: io.text_encoding(encoding=None))
check(TypeError, "'str' object cannot be interpreted as an integer", lambda: io.text_encoding(None, 'x'))
with open(root / 'io_text.txt', encoding=io.text_encoding(None)) as f:
    assert f.read() == 'one\ntwo\n'

# === json ===
s = StringIO()
assert json.dump({'a': [1, 2]}, s) is None
assert s.getvalue() == '{"a": [1, 2]}'
assert json.load(StringIO('{"b": 3}')) == {'b': 3}
s = StringIO('  [1, 2]  ')
s.read(2)
assert json.load(s) == [1, 2]
check(TypeError, "a bytes-like object is required, not 'str'", lambda: json.dump([1], BytesIO()))
assert json.load(BytesIO(b'{"c": 4}')) == {'c': 4}
closed = StringIO()
closed.close()
check(ValueError, 'I/O operation on closed file', lambda: json.dump([1], closed))
check(ValueError, 'I/O operation on closed file', lambda: json.load(closed))
# a container's first chunk meets the stream's checks before a bad element is encoded,
# while a scalar is encoded whole first
check(TypeError, "a bytes-like object is required, not 'str'", lambda: json.dump({'a': {1}}, BytesIO()))
check(ValueError, 'I/O operation on closed file', lambda: json.dump([{1}], closed))
check(TypeError, 'Object of type set is not JSON serializable', lambda: json.dump({1}, BytesIO()))

# === csv ===
out = StringIO()
w = csv.writer(out)
assert w.writerow(['a', 'b,c']) == 9
assert w.writerows([[1, 2], [3, 4]]) is None
assert out.getvalue() == 'a,"b,c"\r\n1,2\r\n3,4\r\n'
assert list(csv.reader(StringIO('x,y\n1,2\n'))) == [['x', 'y'], ['1', '2']]
assert list(csv.DictReader(StringIO('x,y\n1,2\n'))) == [{'x': '1', 'y': '2'}]
out = StringIO()
dw = csv.DictWriter(out, fieldnames=['x', 'y'])
dw.writeheader()
dw.writerows([{'x': 1, 'y': 2}])
assert out.getvalue() == 'x,y\r\n1,2\r\n'
check(TypeError, "a bytes-like object is required, not 'str'", lambda: csv.writer(BytesIO()).writerow(['a']))
assert csv.writer(BytesIO()).writerows([]) is None


class FailingRows:
    def __init__(self):
        self.n = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.n += 1
        if self.n > 2:
            raise ValueError('boom')
        return {'x': self.n, 'y': 0}


# a stream is written row by row, so rows before a failure stay written
out = StringIO()
try:
    csv.writer(out).writerows([[1], [2], 1])
    assert False, 'expected csv.Error'
except csv.Error:
    pass
assert out.getvalue() == '1\r\n2\r\n'
out = StringIO()
try:
    csv.DictWriter(out, fieldnames=['x', 'y']).writerows(FailingRows())
    assert False, 'expected ValueError'
except ValueError:
    pass
assert out.getvalue() == '1,0\r\n2,0\r\n'

# === print ===
buf = StringIO()
assert print('hello', 'world', sep='-', end='!\n', file=buf) is None
print(1, file=buf)
print(file=buf)
assert buf.getvalue() == 'hello-world!\n1\n\n'
check(TypeError, "a bytes-like object is required, not 'str'", lambda: print('x', file=BytesIO()))
check(ValueError, 'I/O operation on closed file', lambda: print('x', file=closed))


class Explodes:
    def __str__(self):
        raise RuntimeError('boom')


# pieces written before a failing `str()` stay written, as CPython writes each in turn
buf = StringIO()
try:
    print('first', Explodes(), file=buf)
    assert False, 'expected RuntimeError'
except RuntimeError:
    pass
assert buf.getvalue() == 'first '
