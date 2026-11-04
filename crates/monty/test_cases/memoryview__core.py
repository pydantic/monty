import base64
import hashlib

data = bytearray(b'abcdef')
v = memoryview(data)
assert v.obj is data
assert v.format == 'B' and v.itemsize == 1 and v.ndim == 1
assert v.shape == (6,) and v.strides == (1,) and v.suboffsets == ()
assert v.nbytes == 6 and len(v) == 6 and v.contiguous
assert not v.readonly
v[0] = 65
assert data == b'Abcdef'
v[1:3] = b'BC'
assert data == b'ABCdef'
w = v[::-2]
assert w.tolist() == [102, 100, 66]
assert not w.contiguous and w.strides == (-2,)
w[:] = b'XYZ'
assert data == b'AZCYeX'
assert bytes(w) == b'XYZ'
assert w.tobytes() == b'XYZ'
assert memoryview(v).obj is data
r = w.toreadonly()
assert r.readonly
try:
    r[0] = 0
except TypeError as e:
    assert str(e) == 'cannot modify read-only memory'
else:
    assert False
for operation in ['append', 'clear', 'resize', 'pop', 'remove', 'insert', 'extend', 'slice', 'repeat']:
    try:
        if operation == 'append':
            data.append(0)
        elif operation == 'clear':
            data.clear()
        elif operation == 'resize':
            data.resize(7)
        elif operation == 'pop':
            data.pop()
        elif operation == 'remove':
            data.remove(65)
        elif operation == 'insert':
            data.insert(0, 0)
        elif operation == 'extend':
            data.extend(b'x')
        elif operation == 'slice':
            data[:] = b'x'
        else:
            data *= 2
    except BufferError as e:
        assert str(e) == 'Existing exports of data: object cannot be re-sized'
    else:
        assert False, operation
assert data == b'AZCYeX'
data.resize(6)
data *= 1
data.extend(b'')
v.release()
v.release()
for operation in ['len', 'bool', 'obj', 'get', 'bytes', 'list', 'iter']:
    try:
        if operation == 'len':
            len(v)
        elif operation == 'bool':
            bool(v)
        elif operation == 'obj':
            v.obj
        elif operation == 'get':
            v[0]
        elif operation == 'bytes':
            v.tobytes()
        elif operation == 'list':
            v.tolist()
        else:
            iter(v)
    except ValueError as e:
        assert str(e) == 'operation forbidden on released memoryview object'
    else:
        assert False, operation
assert w.tobytes() == b'XYZ'
w.release()
r.release()
data.append(0)
assert data == b'AZCYeX\0'
with memoryview(data) as managed:
    managed[-1] = 33
assert data == b'AZCYeX!'
data.append(0)
assert list(memoryview(b'abc')) == [97, 98, 99]
assert memoryview(b'abc').tobytes(order='F') == b'abc'
assert memoryview(b'abc').hex(':') == '61:62:63'
assert hash(memoryview(b'abc')) == hash(b'abc')
assert memoryview(b'abc') == b'abc'
assert memoryview(b'abc') == bytearray(b'abc')
assert memoryview(b'\xff').cast('b').tolist() == [-1]
assert memoryview(b'\xff').cast('b') != b'\xff'
assert memoryview(b'a').cast('c').tolist() == [b'a']
assert memoryview(b'a').cast('c') != b'a'
signed = memoryview(bytearray(b'\x00')).cast('b')
signed[0] = -1
assert signed.tobytes() == b'\xff'
signed.release()

assert b':'.join([memoryview(b'a'), bytearray(b'b')]) == b'a:b'
assert b'abc'.startswith(memoryview(b'ab'))
assert b'abc'.find(memoryview(b'bc')) == 1
assert hashlib.sha256(memoryview(b'abc')).digest() == hashlib.sha256(b'abc').digest()
assert base64.b64encode(memoryview(b'abc')) == b'YWJj'
assert base64.b32encode(memoryview(b'abcd')[::2]) == b'MFRQ===='
assert b'%b' % memoryview(b'abcd')[::2] == b'ac'
assert bytearray(memoryview(b'abcd')[::2]) == b'ac'
assert bytearray.fromhex(memoryview(b'41 42')) == b'AB'
assert memoryview(b'a').cast('@B').format == '@B'
for consume in ['join', 'hash', 'fromhex', 'base64']:
    try:
        strided = memoryview(b'1234')[::2]
        if consume == 'join':
            b''.join([strided])
        elif consume == 'hash':
            hashlib.sha256(strided)
        elif consume == 'fromhex':
            bytearray.fromhex(strided)
        else:
            base64.b64encode(strided)
    except (BufferError, TypeError) as exc:
        assert isinstance(exc, TypeError if consume == 'join' else BufferError)
    else:
        assert False, consume

assert memoryview(b'ababa').count(97) == 3
assert memoryview(b'ababa').index(97, 1) == 2
assert memoryview(b'ababa')[::-1].index(98) == 1
try:
    memoryview(b'abc').index(255)
except ValueError as e:
    assert str(e) == 'memoryview.index(x): x not found'
else:
    assert False

assert memoryview(b'a')[:: -(2**63)].tobytes() == b'a'
assert memoryview(b'ab')[1 :: (2**63 - 1)][1:1].tobytes() == b''

v = memoryview(b'a')
v.release()
try:
    v.cast(['B'])
except TypeError as e:
    assert str(e) == "cast() argument 'format' must be str, not list"
else:
    assert False

assert memoryview[int].__origin__ is memoryview

view = memoryview(b'abc')
expected_hash = hash(view)
view.release()
assert hash(view) == expected_hash
