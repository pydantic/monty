import base64
import binascii
import bisect
import random


def expect_error(function, kind, message):
    try:
        function()
    except Exception as exc:
        assert isinstance(exc, kind)
        assert str(exc) == message, (str(exc), message)
    else:
        assert False


zero = memoryview(b'ab').cast('H', [])
for consumer in [
    lambda value: base64.b64encode(b'abc', altchars=value),
    lambda value: binascii.hexlify(b'a', sep=value),
    lambda value: random.Random(3).choice(value),
    lambda value: bisect.bisect_left(value, 0),
]:
    expect_error(lambda: consumer(zero), TypeError, '0-dim memory has no length')
    for raw in [b'a', b'ab', b'abcdefgh']:
        released = memoryview(raw)
        released.release()
        expect_error(lambda: consumer(released), ValueError, 'operation forbidden on released memoryview object')

for value in [memoryview(b'abcd').cast('H'), memoryview(b'abcd').cast('B', [2, 2])]:
    expect_error(
        lambda: base64.b64encode(b'abc', altchars=value), ValueError, 'maketrans arguments must have same length'
    )

expect_error(lambda: base64.b64decode(b'YWJj', altchars=bytearray(b'abc')), AssertionError, "bytearray(b'abc')")
for view in [memoryview(b'4142')[::2], memoryview(b'4142')]:
    if view.contiguous:
        view.release()
    expect_error(
        lambda: binascii.unhexlify(view),
        TypeError,
        "argument should be bytes, buffer or ASCII string, not 'memoryview'",
    )

expect_error(lambda: base64.b32decode(b'MY======', map01=bytearray(b'IL')), AssertionError, "bytearray(b'IL')")
assert base64.b32decode(b'MY======', map01=bytearray(b'I')) == b'f'

for function in [base64.encodebytes, base64.decodebytes]:
    for code in ['H', '@B', '@c']:
        view = memoryview(b'abcd').cast(code)
        expect_error(
            lambda: function(view), TypeError, "expected single byte elements, not '" + code + "' from memoryview"
        )
    for view in [memoryview(b'a').cast('B', []), memoryview(b'abcd').cast('B', [2, 2])]:
        expect_error(lambda: function(view), TypeError, f'expected 1-D data, not {view.ndim}-D data from memoryview')
strided = memoryview(b'4142')[::2]
expect_error(lambda: base64.encodebytes(strided), BufferError, 'memoryview: underlying buffer is not C-contiguous')
expect_error(
    lambda: base64.decodebytes(strided),
    TypeError,
    "argument should be bytes, buffer or ASCII string, not 'memoryview'",
)
empty = memoryview(b'ab')[0:0:2]
assert not empty.contiguous
assert base64.encodebytes(empty) == b''

# Search bounds are converted before dimensional lookup is checked.
for shape in [[], [1], [1, 1]]:
    view = memoryview(b'x').cast('B', shape)
    for bound in [None, 1.5]:
        try:
            view.index(0, bound)
        except TypeError as exc:
            assert str(exc) == 'slice indices must be integers or have an __index__ method'
        else:
            assert False

try:
    0 in memoryview(b'x').cast('B', [])
except TypeError as exc:
    assert str(exc) == "argument of type 'memoryview' is not a container or iterable"
else:
    assert False

released = memoryview(b'x')
released.release()
try:
    released.index(0, None)
except TypeError as exc:
    assert str(exc) == 'slice indices must be integers or have an __index__ method'
else:
    assert False

for wrong in [None, 1, 'x', [], {}]:
    try:
        memoryview(bytearray(1))[:] = wrong
    except TypeError as exc:
        assert str(exc) == f"a bytes-like object is required, not '{type(wrong).__name__}'"
    else:
        assert False

for size in range(3):
    for consumed in range(size + 1):
        view = memoryview(bytes(size))
        iterator = iter(view)
        for _ in range(consumed):
            next(iterator)
        view.release()
        try:
            next(iterator)
        except StopIteration:
            assert consumed == size
        except ValueError as exc:
            assert consumed < size
            assert str(exc) == 'operation forbidden on released memoryview object'
        else:
            assert False
