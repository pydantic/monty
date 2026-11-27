import copy
import io
from io import StringIO


def check(exc_type, message, fn):
    try:
        fn()
    except exc_type as e:
        assert type(e) is exc_type
        assert str(e) == message
    else:
        assert False, 'did not raise'


def closed_stream():
    s = StringIO('abc')
    s.close()
    return s


# === construction ===
assert StringIO().getvalue() == ''
assert StringIO('hello').getvalue() == 'hello'
assert StringIO(None).getvalue() == ''
assert StringIO(initial_value='kw').getvalue() == 'kw'
assert StringIO('x', None).getvalue() == 'x'
assert StringIO('starts at zero').tell() == 0
assert type(StringIO()) is io.StringIO
assert repr(type(StringIO())) == "<class '_io.StringIO'>"
assert repr(StringIO()).startswith('<_io.StringIO object at 0x')
assert bool(StringIO())
s = StringIO()
assert s == s
assert s != StringIO()
assert hash(s) == hash(s)
check(TypeError, 'initial_value must be str or None, not int', lambda: StringIO(1))
check(TypeError, 'initial_value must be str or None, not bytes', lambda: StringIO(b'x'))
check(TypeError, 'newline must be str or None, not int', lambda: StringIO('', 1))
check(TypeError, 'newline must be str or None, not bytes', lambda: StringIO('', newline=b'\n'))
check(ValueError, "illegal newline value: 'x'", lambda: StringIO('', 'x'))
# the newline is validated before the initial value
check(TypeError, 'newline must be str or None, not int', lambda: StringIO(1, 1))
check(ValueError, "illegal newline value: 'x'", lambda: StringIO(1, 'x'))
check(TypeError, 'StringIO() takes at most 2 arguments (3 given)', lambda: StringIO('', None, 1))
check(TypeError, 'StringIO() takes at most 2 arguments (3 given)', lambda: StringIO('', None, foo=1))
check(TypeError, "StringIO() got an unexpected keyword argument 'foo'", lambda: StringIO(foo=1))
check(TypeError, "StringIO() got an unexpected keyword argument 'foo'", lambda: StringIO('', foo=1))
check(
    TypeError,
    "argument for StringIO() given by name ('initial_value') and position (1)",
    lambda: StringIO('', initial_value=''),
)

# `__init__` resets the stream, including its newline mode, and reopens a closed one
s = StringIO('abc')
s.read(1)
s.close()
assert s.__init__('x\ny', newline='') is None
assert (s.closed, s.tell(), s.getvalue(), s.newlines) == (False, 0, 'x\ny', '\n')
s.seek(2)
check(TypeError, 'initial_value must be str or None, not list', lambda: s.__init__([]))
# a rejected value leaves the stream as it was
assert (s.tell(), s.getvalue()) == (2, 'x\ny')

# === read ===
s = StringIO('hello world')
assert s.read(5) == 'hello'
assert s.tell() == 5
assert s.read() == ' world'
assert s.read() == ''
assert s.read(3) == ''
s.seek(0)
assert s.read(-1) == 'hello world'
s.seek(0)
assert s.read(None) == 'hello world'
s.seek(6)
assert s.read(0) == ''
assert s.read(100) == 'world'
assert StringIO('abc').read(True) == 'a'
check(TypeError, "argument should be integer or None, not 'float'", lambda: StringIO('abc').read(1.0))
check(TypeError, "argument should be integer or None, not 'str'", lambda: StringIO('abc').read('1'))
check(OverflowError, "cannot fit 'int' into an index-sized integer", lambda: StringIO('a').read(2**70))
check(TypeError, 'read expected at most 1 argument, got 2', lambda: StringIO('abc').read(1, 2))
check(TypeError, 'StringIO.read() takes no keyword arguments', lambda: StringIO('abc').read(size=1))

# === non-ASCII positions count chars ===
s = StringIO('héllo wörld ☃')
assert s.read(2) == 'hé'
assert s.tell() == 2
assert s.read(4) == 'llo '
s.seek(7)
assert s.read(1) == 'ö'
s.seek(0, 2)
assert s.tell() == 13
s.seek(1)
s.write('E')
assert s.getvalue() == 'hEllo wörld ☃'
s.seek(12)
s.write('☃☃')
assert s.getvalue() == 'hEllo wörld ☃☃'
assert s.tell() == 14

# === readline and readlines ===
s = StringIO('one\ntwo\nthree')
assert s.readline() == 'one\n'
assert s.readline() == 'two\n'
assert s.readline() == 'three'
assert s.readline() == ''
assert StringIO('abcd\nef').readline(2) == 'ab'
assert StringIO('abcd\nef').readline(-1) == 'abcd\n'
assert StringIO('abcd\nef').readline(None) == 'abcd\n'
# a size bounds the newline search, so a `\r\n` it splits ends the line at the `\r`
assert StringIO('ab\r\ncd', newline='').readline(3) == 'ab\r'
assert StringIO('ab\r\ncd', newline='').readline(4) == 'ab\r\n'
s = StringIO(newline='\r\n')
s.write('ab\ncd')
s.seek(0)
assert s.readline(3) == 'ab\r'
assert s.readline() == '\ncd'
assert StringIO('é€x\ny').readline(2) == 'é€'
assert StringIO('é\nx').readline(5) == 'é\n'
assert StringIO('ab\ncd').readline(0) == ''
# repeated short reads of a buffer with no newline stay linear
s = StringIO('é' * 100_000)
lines = 0
while s.readline(1):
    lines += 1
assert lines == 100_000
assert StringIO('ab\ncd\nef\n').readlines() == ['ab\n', 'cd\n', 'ef\n']
# `IOBase.readlines` stops after the line that takes the total past the hint
assert StringIO('ab\ncd\nef\n').readlines(3) == ['ab\n', 'cd\n']
assert StringIO('ab\ncd\nef\n').readlines(2) == ['ab\n']
assert StringIO('ab\ncd\nef\n').readlines(0) == ['ab\n', 'cd\n', 'ef\n']
assert StringIO('a\nb').readlines(-3) == ['a\n', 'b']
assert StringIO('ab\ncd\nef\n').readlines(None) == ['ab\n', 'cd\n', 'ef\n']
check(TypeError, "argument should be integer or None, not 'str'", lambda: StringIO('ab').readline('x'))
check(TypeError, "argument should be integer or None, not 'str'", lambda: StringIO('ab').readlines('x'))
check(OverflowError, "cannot fit 'int' into an index-sized integer", lambda: StringIO('a').readline(2**70))
check(TypeError, 'StringIO.readline() takes no keyword arguments', lambda: StringIO().readline(size=1))
check(TypeError, '_IOBase.readlines() takes no keyword arguments', lambda: StringIO().readlines(hint=1))

# === iteration ===
assert list(StringIO('a\nb\nc')) == ['a\n', 'b\n', 'c']
assert list(StringIO('')) == []
s = StringIO('x\ny\n')
assert iter(s) is s
assert next(s) == 'x\n'
assert [line for line in s] == ['y\n']
assert sorted(StringIO('b\na\n')) == ['a\n', 'b\n']
assert ''.join(StringIO('x\ny')) == 'x\ny'
assert [(i, line) for i, line in enumerate(StringIO('p\nq'))] == [(0, 'p\n'), (1, 'q')]
check(StopIteration, '', lambda: next(StringIO()))


# a stream is an iterator, so `__iter__` may return one
class Lines:
    def __iter__(self):
        return StringIO('x\ny')


assert list(Lines()) == ['x\n', 'y']

# === write ===
s = StringIO()
assert s.write('hello') == 5
assert s.write(' ☃') == 2
assert s.getvalue() == 'hello ☃'
assert s.tell() == 7
assert s.write('') == 0
s = StringIO('hello')
s.seek(1)
assert s.write('EE') == 2
assert s.getvalue() == 'hEElo'
s.seek(0, 2)
s.write('!')
assert s.getvalue() == 'hEElo!'
# writing past the end pads with NUL chars; an empty write does not
s = StringIO('ab')
s.seek(5)
assert s.write('') == 0
assert s.getvalue() == 'ab'
s.write('x')
assert s.getvalue() == 'ab\x00\x00\x00x'
assert s.tell() == 6
check(TypeError, "string argument expected, got 'bytes'", lambda: StringIO().write(b'x'))
check(TypeError, "string argument expected, got 'int'", lambda: StringIO().write(1))
check(TypeError, "string argument expected, got 'bool'", lambda: StringIO().write(True))
check(TypeError, 'StringIO.write() takes exactly one argument (0 given)', lambda: StringIO().write())
check(TypeError, 'StringIO.write() takes no keyword arguments', lambda: StringIO().write(s='x'))

# === writelines ===
s = StringIO()
assert s.writelines(['a', 'b\n', 'c']) is None
assert s.getvalue() == 'ab\nc'
s.writelines(x for x in 'de')
s.writelines('fg')
assert s.getvalue() == 'ab\ncdefg'
check(TypeError, "string argument expected, got 'int'", lambda: StringIO().writelines([1]))
check(TypeError, "'int' object is not iterable", lambda: StringIO().writelines(1))
check(TypeError, '_IOBase.writelines() takes exactly one argument (0 given)', lambda: StringIO().writelines())
check(TypeError, '_IOBase.writelines() takes no keyword arguments', lambda: StringIO().writelines(lines=[]))

# === seek and tell ===
s = StringIO('abcdef')
assert s.seek(3) == 3
assert s.read(1) == 'd'
assert s.seek(0, 1) == 4
assert s.seek(0, 2) == 6
assert s.seek(0, io.SEEK_SET) == 0
assert s.seek(True) == 1
assert s.seek(False, True) == 1
assert s.seek(10) == 10
assert s.read() == ''
assert s.tell() == 10
assert s.getvalue() == 'abcdef'
check(ValueError, 'Negative seek position -1', lambda: StringIO('a').seek(-1))
check(OSError, "Can't do nonzero cur-relative seeks", lambda: StringIO('a').seek(1, 1))
check(OSError, "Can't do nonzero cur-relative seeks", lambda: StringIO('a').seek(-1, 2))
check(ValueError, 'Invalid whence (3, should be 0, 1 or 2)', lambda: StringIO('a').seek(0, 3))
check(TypeError, "'float' object cannot be interpreted as an integer", lambda: StringIO('a').seek(1.0))
check(TypeError, "'float' object cannot be interpreted as an integer", lambda: StringIO('a').seek(0, 1.0))
check(OverflowError, 'Python int too large to convert to C ssize_t', lambda: StringIO('a').seek(2**70))
check(OverflowError, 'Python int too large to convert to C int', lambda: StringIO('a').seek(0, 2**40))
check(TypeError, 'seek expected at least 1 argument, got 0', lambda: StringIO('a').seek())
check(TypeError, 'seek expected at most 2 arguments, got 3', lambda: StringIO('a').seek(1, 0, 0))
check(TypeError, 'StringIO.seek() takes no keyword arguments', lambda: StringIO('a').seek(pos=1))
check(TypeError, 'StringIO.tell() takes no arguments (1 given)', lambda: StringIO().tell(1))
s = StringIO()
s.seek(2**63 - 1)
check(OverflowError, 'new position too large', lambda: s.write('x'))
assert s.write('') == 0

# === truncate ===
s = StringIO('hello')
s.seek(2)
assert s.truncate() == 2
assert s.getvalue() == 'he'
assert s.tell() == 2
assert s.truncate(10) == 10
assert s.getvalue() == 'he'
assert s.truncate(None) == 2
s = StringIO('abcdef')
s.seek(5)
assert s.truncate(2) == 2
assert s.tell() == 5
s.write('x')
assert s.getvalue() == 'ab\x00\x00\x00x'
s = StringIO('ünïcödé')
assert s.truncate(3) == 3
assert s.getvalue() == 'ünï'
check(ValueError, 'Negative size value -1', lambda: StringIO('abc').truncate(-1))
check(TypeError, "argument should be integer or None, not 'str'", lambda: StringIO('abc').truncate('1'))
check(OverflowError, "cannot fit 'int' into an index-sized integer", lambda: StringIO('a').truncate(2**70))
check(TypeError, 'StringIO.truncate() takes no keyword arguments', lambda: StringIO().truncate(size=1))

# === newline translation ===
assert StringIO('a\r\nb\rc\n', newline=None).getvalue() == 'a\nb\nc\n'
assert StringIO('a\r\nb\rc\n', newline='').getvalue() == 'a\r\nb\rc\n'
assert StringIO('a\r\nb', newline='\n').getvalue() == 'a\r\nb'
assert StringIO('a\nb', newline='\r').getvalue() == 'a\rb'
assert StringIO('a\nb', newline='\r\n').getvalue() == 'a\r\nb'
assert StringIO(newline='\r\n').write('a\nb') == 3
s = StringIO(newline=None)
assert s.write('a\r\n') == 3
assert s.tell() == 2
# each write is translated on its own, so a split `\r\n` becomes two newlines
s = StringIO(newline=None)
s.write('a\r')
s.write('\nb')
assert s.getvalue() == 'a\n\nb'
assert s.newlines == ('\r', '\n')


def lines(newline, data):
    s = StringIO(newline=newline)
    s.write(data)
    s.seek(0)
    return [s.readline() for _ in range(5)], s.newlines


assert lines(None, 'a\r\nb\rc\nd') == (['a\n', 'b\n', 'c\n', 'd', ''], ('\r', '\n', '\r\n'))
assert lines('', 'a\r\nb\rc\nd') == (['a\r\n', 'b\r', 'c\n', 'd', ''], ('\r', '\n', '\r\n'))
assert lines('\n', 'a\r\nb\rc\nd') == (['a\r\n', 'b\rc\n', 'd', '', ''], None)
assert lines('\r', 'a\r\nb\rc\nd') == (['a\r', '\r', 'b\r', 'c\r', 'd'], None)
assert lines('\r\n', 'a\r\nb\rc\nd') == (['a\r\r\n', 'b\rc\r\n', 'd', '', ''], None)
assert StringIO('\r\n', newline='').readline(1) == '\r'
assert StringIO('a\n').newlines is None
assert StringIO('a\r\n', newline=None).newlines == '\r\n'
assert StringIO('a\r\nb\n', newline='').newlines == ('\n', '\r\n')
assert StringIO(newline=None).newlines is None

# === attributes and capability checks ===
s = StringIO()
assert s.readable() is True
assert s.writable() is True
assert s.seekable() is True
assert s.isatty() is False
assert s.closed is False
assert s.line_buffering is False
assert s.encoding is None
assert s.errors is None
assert s.flush() is None
check(io.UnsupportedOperation, 'fileno', lambda: StringIO().fileno())
check(io.UnsupportedOperation, 'detach', lambda: StringIO().detach())
check(TypeError, 'fileno() takes no arguments', lambda: StringIO().fileno(1))
check(TypeError, 'detach() takes no arguments', lambda: StringIO().detach(x=1))
check(
    TypeError,
    'object.__init__() takes exactly one argument (the instance to initialize)',
    lambda: object.__init__(StringIO(), 1),
)
check(AttributeError, "'_io.StringIO' object has no attribute 'buffer'", lambda: StringIO().buffer)
check(AttributeError, "'_io.StringIO' object has no attribute 'name'", lambda: StringIO().name)
check(AttributeError, "'_io.StringIO' object has no attribute 'nope'", lambda: StringIO().nope())
check(TypeError, 'StringIO.getvalue() takes no arguments (1 given)', lambda: StringIO().getvalue(1))
check(TypeError, 'StringIO.close() takes no arguments (1 given)', lambda: StringIO().close(1))
check(TypeError, 'StringIO.readable() takes no arguments (1 given)', lambda: StringIO().readable(1))
check(TypeError, '_IOBase.isatty() takes no arguments (1 given)', lambda: StringIO().isatty(1))
check(TypeError, '_IOBase.flush() takes no arguments (1 given)', lambda: StringIO().flush(1))
try:
    StringIO().fileno()
    assert False, 'expected UnsupportedOperation'
except OSError:
    pass
try:
    StringIO().detach()
    assert False, 'expected UnsupportedOperation'
except ValueError:
    pass

# === closing ===
s = StringIO('abc')
assert s.close() is None
assert s.closed is True
assert s.close() is None
# `StringIO`'s own methods omit the full stop that the `IOBase` ones keep
check(ValueError, 'I/O operation on closed file', lambda: closed_stream().read())
check(ValueError, 'I/O operation on closed file', lambda: closed_stream().readline())
check(ValueError, 'I/O operation on closed file', lambda: closed_stream().write('x'))
check(ValueError, 'I/O operation on closed file', lambda: closed_stream().getvalue())
check(ValueError, 'I/O operation on closed file', lambda: closed_stream().tell())
check(ValueError, 'I/O operation on closed file', lambda: closed_stream().seek(0))
check(ValueError, 'I/O operation on closed file', lambda: closed_stream().seek(0, 5))
check(ValueError, 'I/O operation on closed file', lambda: closed_stream().truncate())
check(ValueError, 'I/O operation on closed file', lambda: closed_stream().readable())
check(ValueError, 'I/O operation on closed file', lambda: closed_stream().writable())
check(ValueError, 'I/O operation on closed file', lambda: closed_stream().seekable())
check(ValueError, 'I/O operation on closed file', lambda: next(closed_stream()))
check(ValueError, 'I/O operation on closed file', lambda: closed_stream().newlines)
check(ValueError, 'I/O operation on closed file', lambda: closed_stream().line_buffering)
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().readlines())
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().writelines([]))
check(ValueError, 'I/O operation on closed file.', lambda: closed_stream().isatty())
check(ValueError, 'I/O operation on closed file.', lambda: iter(closed_stream()))
# argument errors come before the closed check, except write's type check
check(TypeError, "argument should be integer or None, not 'str'", lambda: closed_stream().read('x'))
check(TypeError, "string argument expected, got 'int'", lambda: closed_stream().write(1))
assert closed_stream().flush() is None
assert closed_stream().encoding is None
check(io.UnsupportedOperation, 'fileno', lambda: closed_stream().fileno())

# === context manager ===
with StringIO('ctx') as s:
    assert s.read() == 'ctx'
assert s.closed
try:
    with closed_stream():
        pass
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'I/O operation on closed file.'
try:
    with StringIO() as s:
        raise KeyError('inner')
except KeyError:
    assert s.closed

# === copy ===
s = StringIO('ab\r\n', newline='')
s.read(1)
c = copy.copy(s)
assert c is not s
assert c.getvalue() == 'ab\r\n'
assert c.tell() == 1
assert c.newlines == '\r\n'
c.write('X')
assert s.getvalue() == 'ab\r\n'
assert copy.deepcopy(s).getvalue() == 'ab\r\n'
# the copy's `newlines` reflects its stored text, already translated here
s = StringIO(newline=None)
s.write('a\r\nb\r')
assert s.newlines == ('\r', '\r\n')
assert copy.copy(s).newlines == '\n'
d = copy.deepcopy([s, s])
assert d[0] is d[1]
check(ValueError, 'I/O operation on closed file', lambda: copy.copy(closed_stream()))
check(ValueError, 'I/O operation on closed file', lambda: copy.deepcopy(closed_stream()))

# === unbound methods ===
s = StringIO('xy\nz')
assert StringIO.readline(s) == 'xy\n'
assert StringIO.write(s, '!') == 1 and StringIO.getvalue(s) == 'xy\n!'
assert StringIO.__init__(s, 'reset') is None and s.getvalue() == 'reset'
check(
    TypeError,
    "descriptor 'getvalue' for '_io.StringIO' objects doesn't apply to a '_io.BytesIO' object",
    lambda: StringIO.getvalue(io.BytesIO()),
)
check(
    TypeError, "descriptor 'tell' for '_io.StringIO' objects doesn't apply to a 'int' object", lambda: StringIO.tell(1)
)
