import copy
import io
from io import BytesIO


import sys


def _error_message(*, monty, cpython):
    return monty if sys.platform == 'monty' else cpython


def check(exc_type, message, fn):
    try:
        fn()
    except exc_type as e:
        assert type(e) is exc_type
        assert str(e) == message
    else:
        assert False, 'did not raise'


def closed_stream():
    b = BytesIO(b'abc')
    b.close()
    return b


# === construction ===
assert BytesIO().getvalue() == b''
assert BytesIO(b'hello').getvalue() == b'hello'
assert BytesIO(None).getvalue() == b''
assert BytesIO(initial_bytes=b'kw').getvalue() == b'kw'
assert BytesIO(b'xyz').tell() == 0
assert type(BytesIO()) is io.BytesIO
assert repr(type(BytesIO())) == "<class '_io.BytesIO'>"
assert repr(BytesIO()).startswith('<_io.BytesIO object at 0x')
b = BytesIO()
assert b == b
assert b != BytesIO()
assert hash(b) == hash(b)
check(TypeError, "a bytes-like object is required, not 'str'", lambda: BytesIO('x'))
check(TypeError, "a bytes-like object is required, not 'int'", lambda: BytesIO(1))
check(TypeError, "a bytes-like object is required, not 'list'", lambda: BytesIO([1]))
check(TypeError, 'BytesIO() takes at most 1 argument (2 given)', lambda: BytesIO(b'', b''))
check(
    TypeError,
    _error_message(
        monty="BytesIO() got an unexpected keyword argument 'foo'",
        cpython='BytesIO() takes at most 1 argument (2 given)',
    ),
    lambda: BytesIO(b'', foo=1),
)
check(TypeError, "BytesIO() got an unexpected keyword argument 'foo'", lambda: BytesIO(foo=1))

# `__init__` resets the stream and reopens a closed one
b = BytesIO(b'abc')
b.read(1)
b.close()
assert b.__init__(b'xyz') is None
assert (b.closed, b.tell(), b.getvalue()) == (False, 0, b'xyz')
b.seek(2)
check(TypeError, "a bytes-like object is required, not 'list'", lambda: b.__init__([]))
# the stream is emptied before the payload is checked
assert (b.closed, b.tell(), b.getvalue()) == (False, 0, b'')

# === read and read1 ===
b = BytesIO(b'hello world')
assert b.read(5) == b'hello'
assert b.tell() == 5
assert b.read() == b' world'
assert b.read() == b''
b.seek(0)
assert b.read(-5) == b'hello world'
b.seek(0)
assert b.read(None) == b'hello world'
b.seek(6)
assert b.read(0) == b''
assert b.read1(2) == b'wo'
assert b.read1() == b'rld'
assert BytesIO(b'abc').read1(-1) == b'abc'
check(TypeError, "argument should be integer or None, not 'float'", lambda: BytesIO(b'abc').read(1.0))
check(TypeError, "argument should be integer or None, not 'str'", lambda: BytesIO(b'abc').read('1'))
check(OverflowError, "cannot fit 'int' into an index-sized integer", lambda: BytesIO(b'a').read(2**70))
check(TypeError, 'read expected at most 1 argument, got 2', lambda: BytesIO().read(1, 2))
check(TypeError, 'BytesIO.read() takes no keyword arguments', lambda: BytesIO().read(size=1))
check(TypeError, 'BytesIO.read1() takes no keyword arguments', lambda: BytesIO().read1(size=1))

# === readline, readlines and iteration ===
b = BytesIO(b'one\ntwo\r\nthree\rfour')
assert b.readline() == b'one\n'
assert b.readline() == b'two\r\n'
assert b.readline() == b'three\rfour'
assert b.readline() == b''
assert BytesIO(b'abcd\nef').readline(2) == b'ab'
assert BytesIO(b'a\nb').readline(None) == b'a\n'
assert BytesIO(b'ab\ncd').readline(3) == b'ab\n'
assert BytesIO(b'ab\ncd').readline(0) == b''
assert BytesIO(b'ab').readline(9) == b'ab'
# repeated short reads of a buffer with no newline stay linear
b = BytesIO(b'x' * 100_000)
lines = 0
while b.readline(1):
    lines += 1
assert lines == 100_000
assert BytesIO(b'ab\ncd\nef\n').readlines() == [b'ab\n', b'cd\n', b'ef\n']
# `BytesIO.readlines` stops at the line that brings the total to the hint
assert BytesIO(b'ab\ncd\nef\n').readlines(3) == [b'ab\n']
assert BytesIO(b'ab\ncd\nef\n').readlines(4) == [b'ab\n', b'cd\n']
assert BytesIO(b'a\nb').readlines(True) == [b'a\n']
assert BytesIO(b'a\nb').readlines(-1) == [b'a\n', b'b']
assert BytesIO(b'a\nb').readlines(None) == [b'a\n', b'b']
check(TypeError, "integer argument expected, got 'str'", lambda: BytesIO(b'ab\n').readlines('x'))
check(TypeError, "integer argument expected, got 'float'", lambda: BytesIO(b'ab\n').readlines(1.0))
check(OverflowError, 'Python int too large to convert to C ssize_t', lambda: BytesIO(b'a').readlines(2**70))
check(OverflowError, "cannot fit 'int' into an index-sized integer", lambda: BytesIO(b'a').readline(2**70))
check(TypeError, 'BytesIO.readline() takes no keyword arguments', lambda: BytesIO().readline(size=1))
check(TypeError, 'BytesIO.readlines() takes no keyword arguments', lambda: BytesIO().readlines(hint=1))
assert list(BytesIO(b'a\nb\nc')) == [b'a\n', b'b\n', b'c']
b = BytesIO(b'x\ny')
assert iter(b) is b
assert next(b) == b'x\n'
assert [line for line in b] == [b'y']
check(StopIteration, '', lambda: next(BytesIO()))

# === write and writelines ===
b = BytesIO()
assert b.write(b'hey') == 3
assert b.write(b'') == 0
assert b.getvalue() == b'hey'
b = BytesIO(b'hello')
b.seek(1)
assert b.write(b'EEEEEE') == 6
assert b.getvalue() == b'hEEEEEE'
b.seek(1)
b.write(b'xy')
assert b.getvalue() == b'hxyEEEE'
# writing past the end pads with zero bytes; an empty write does not
b = BytesIO(b'ab')
b.seek(5)
assert b.write(b'') == 0
assert b.getvalue() == b'ab'
b.write(b'x')
assert b.getvalue() == b'ab\x00\x00\x00x'
assert b.tell() == 6
b = BytesIO()
assert b.writelines([b'a', b'b']) is None
assert b.getvalue() == b'ab'
check(TypeError, "a bytes-like object is required, not 'str'", lambda: BytesIO().write('x'))
check(TypeError, "a bytes-like object is required, not 'int'", lambda: BytesIO().write(1))
check(TypeError, "a bytes-like object is required, not 'str'", lambda: BytesIO().writelines(['a']))
check(TypeError, "a bytes-like object is required, not 'int'", lambda: BytesIO().writelines(b'ab'))
check(TypeError, 'BytesIO.write() takes exactly one argument (0 given)', lambda: BytesIO().write())
check(TypeError, 'BytesIO.write() takes no keyword arguments', lambda: BytesIO().write(b=b'x'))
check(TypeError, 'BytesIO.writelines() takes no keyword arguments', lambda: BytesIO().writelines(lines=[]))

# === seek ===
b = BytesIO(b'hello')
assert b.seek(2) == 2
assert b.seek(-2, 2) == 3
assert b.seek(1, 1) == 4
assert b.seek(-100, 1) == 0
assert b.seek(-5, 2) == 0
assert b.seek(10) == 10
assert b.read() == b''
assert b.tell() == 10
assert b.seek(0, io.SEEK_END) == 5
check(ValueError, 'negative seek value -1', lambda: BytesIO(b'a').seek(-1))
check(ValueError, 'invalid whence (3, should be 0, 1 or 2)', lambda: BytesIO(b'a').seek(0, 3))
check(ValueError, 'invalid whence (3, should be 0, 1 or 2)', lambda: BytesIO(b'a').seek(-1, 3))
check(TypeError, "'float' object cannot be interpreted as an integer", lambda: BytesIO(b'a').seek(1.0))
check(OverflowError, 'Python int too large to convert to C ssize_t', lambda: BytesIO(b'a').seek(2**70))
check(OverflowError, 'Python int too large to convert to C int', lambda: BytesIO(b'a').seek(0, 2**40))
check(TypeError, 'seek expected at least 1 argument, got 0', lambda: BytesIO().seek())
check(TypeError, 'BytesIO.seek() takes no keyword arguments', lambda: BytesIO().seek(pos=1))
b = BytesIO(b'a')
b.seek(1)
check(OverflowError, 'new position too large', lambda: b.seek(2**63 - 1, 1))
b = BytesIO()
b.seek(2**63 - 1)
check(OverflowError, 'new buffer size too large', lambda: b.write(b'x'))
assert b.write(b'') == 0

# === truncate ===
b = BytesIO(b'hello')
b.seek(2)
assert b.truncate() == 2
assert b.getvalue() == b'he'
assert b.tell() == 2
assert b.truncate(10) == 10
assert b.getvalue() == b'he'
assert b.truncate(None) == 2
b = BytesIO(b'abcdef')
b.seek(5)
assert b.truncate(2) == 2
assert b.tell() == 5
b.write(b'x')
assert b.getvalue() == b'ab\x00\x00\x00x'
check(ValueError, 'negative size value -1', lambda: BytesIO(b'abc').truncate(-1))
check(TypeError, "'str' object cannot be interpreted as an integer", lambda: BytesIO(b'abc').truncate('1'))
check(TypeError, "'float' object cannot be interpreted as an integer", lambda: BytesIO(b'a').truncate(1.0))
check(OverflowError, 'Python int too large to convert to C long', lambda: BytesIO(b'a').truncate(2**70))
check(TypeError, 'BytesIO.truncate() takes no keyword arguments', lambda: BytesIO().truncate(size=1))

# === attributes and capability checks ===
b = BytesIO()
assert b.readable() is True
assert b.writable() is True
assert b.seekable() is True
assert b.isatty() is False
assert b.flush() is None
assert b.closed is False
check(io.UnsupportedOperation, 'fileno', lambda: BytesIO().fileno())
check(io.UnsupportedOperation, 'detach', lambda: BytesIO().detach())
check(TypeError, 'fileno() takes no arguments', lambda: BytesIO().fileno(1))
check(TypeError, 'detach() takes no arguments', lambda: BytesIO().detach(x=1))
check(
    TypeError,
    'object.__init__() takes exactly one argument (the instance to initialize)',
    lambda: object.__init__(BytesIO(), 1),
)
check(AttributeError, "'_io.BytesIO' object has no attribute 'raw'", lambda: BytesIO().raw)
check(AttributeError, "'_io.BytesIO' object has no attribute 'encoding'", lambda: BytesIO().encoding)
check(TypeError, 'BytesIO.getvalue() takes no arguments (1 given)', lambda: BytesIO().getvalue(1))
check(TypeError, 'BytesIO.flush() takes no arguments (1 given)', lambda: BytesIO().flush(1))
check(TypeError, 'BytesIO.isatty() takes no arguments (1 given)', lambda: BytesIO().isatty(1))
check(TypeError, 'BytesIO.readable() takes no arguments (1 given)', lambda: BytesIO().readable(1))
check(TypeError, 'BytesIO.close() takes no arguments (1 given)', lambda: BytesIO().close(1))
check(TypeError, 'BytesIO.tell() takes no arguments (1 given)', lambda: BytesIO().tell(1))

# === closing ===
b = BytesIO(b'abc')
assert b.close() is None
assert b.closed is True
assert b.close() is None
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().read())
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().read1())
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().readline())
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().readlines())
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().write(b'x'))
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().writelines([]))
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().getvalue())
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().tell())
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().seek(0))
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().truncate())
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().readable())
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().writable())
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().seekable())
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().isatty())
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().flush())
check(ValueError, 'I/O operation on closed file.', lambda: next(closed_stream()))
# `readlines`, `truncate` and `writelines` check for closure before their argument
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().readlines('x'))
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().truncate('x'))
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().writelines(1))
check(TypeError, "argument should be integer or None, not 'str'", lambda: closed_stream().read('x'))
check(TypeError, "'str' object cannot be interpreted as an integer", lambda: closed_stream().seek('x'))
check(TypeError, "a bytes-like object is required, not 'int'", lambda: closed_stream().write(1))
check(io.UnsupportedOperation, 'fileno', lambda: closed_stream().fileno())
b = closed_stream()
assert iter(b) is b

# === context manager ===
with BytesIO(b'ctx') as b:
    assert b.read() == b'ctx'
assert b.closed
try:
    with closed_stream():
        pass
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'I/O operation on closed file.'

# === copy ===
b = BytesIO(b'abc')
b.read(1)
c = copy.copy(b)
assert c is not b
assert c.getvalue() == b'abc'
assert c.tell() == 1
c.write(b'X')
assert b.getvalue() == b'abc'
assert copy.deepcopy(b).getvalue() == b'abc'
check(ValueError, 'I/O operation on closed file.', lambda: copy.copy(closed_stream()))

# === unbound methods ===
b = BytesIO(b'abc')
assert BytesIO.read(b, 2) == b'ab' and BytesIO.read1(b) == b'c'
assert BytesIO.write(b, b'!') == 1 and BytesIO.getvalue(b) == b'abc!'
check(
    TypeError,
    "descriptor 'read' for '_io.BytesIO' objects doesn't apply to a '_io.StringIO' object",
    lambda: BytesIO.read(io.StringIO()),
)
check(TypeError, 'unbound method BytesIO.read() needs an argument', lambda: BytesIO.read())
