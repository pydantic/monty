a = bytearray(b'abc')
assert repr(a) == "bytearray(b'abc')"
assert a == b'abc' and b'abc' == a
assert bytearray(3) == b'\0\0\0'
assert bytearray([65, 66, 255]) == b'AB\xff'
assert bytearray('é', 'utf-8') == b'\xc3\xa9'
assert bytes(a) == b'abc'
assert a[0] == 97
assert isinstance(a[:2], bytearray)
a[0] = 65
assert a == b'Abc'
a[1:2] = b'XYZ'
assert a == b'AXYZc'
a[::-2] = [1, 2, 3]
assert a == bytearray([3, 88, 2, 90, 1])
a[:] = a
assert a == bytearray([3, 88, 2, 90, 1])
a.clear()
a.append(65)
a.extend([66, 67])
a.insert(-1, 90)
assert a == b'ABZC'
assert a.pop() == 67
a.remove(66)
a.reverse()
assert a == b'ZA'
b = a.copy()
a = a + a.copy()
assert a == b'ZAZA' and b == b'ZA'
assert a * 2 == b'ZAZAZAZA'
assert isinstance(a.upper(), bytearray)
assert bytearray.fromhex('41 42') == b'AB'
assert b'Z' in a and bytearray(b'Z') in a
assert b'A' + bytearray(b'B') == b'AB'
it = iter(a)
assert next(it) == 90
a.clear()
assert list(it) == []
a.append(1)
assert list(it) == []

assert all(isinstance(part, bytearray) for part in bytearray(b'a b').split())
assert all(isinstance(part, bytearray) for part in bytearray(b'a:b').partition(b':'))
for bad in [-1, 256]:
    try:
        bytearray([bad])
    except ValueError:
        pass
    else:
        assert False
try:
    hash(bytearray())
except TypeError:
    pass
else:
    assert False

import base64
import hashlib
import json
import random
from copy import copy, deepcopy

assert base64.b64encode(bytearray(b'abc')) == b'YWJj'
assert hashlib.sha256(bytearray(b'abc')).hexdigest() == hashlib.sha256(b'abc').hexdigest()
assert json.loads(bytearray(b'{"x":1}')) == {'x': 1}
assert random.Random(bytearray(b'abc')).random() == random.Random(b'abc').random()
assert b'%b' % bytearray(b'ok') == b'ok'
assert bytearray(b'%b') % b'ok' == bytearray(b'ok')
assert bytearray(b'a') < bytearray(b'b')
a = bytearray(b'abc')
b = copy(a)
c = deepcopy([a, a])
a[0] = 90
assert b == b'abc' and c[0] == b'abc' and c[0] is c[1]

assert type(iter(bytearray())).__name__ == 'bytearray_iterator'

for start in [None, -10, -2, 0, 2, 10]:
    for stop in [None, -10, -2, 0, 2, 10]:
        for step in [-4, -1, 1, 2]:
            key = slice(start, stop, step)
            a = bytearray(b'abcde')
            expected = list(b'abcde')
            assert a[key] == bytearray(expected[key])
            replacement = [90, 91] if step == 1 else [90] * len(expected[key])
            a[key] = replacement
            expected[key] = replacement
            assert a == bytearray(expected)

a = bytearray(b'ab')
a.resize(4)
assert a == b'ab\0\0'
a.resize(1)
assert a == b'a'
