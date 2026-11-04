class Count:
    def __index__(self):
        return 3


class Reflected:
    def __rmul__(self, other):
        return 42


assert bytearray(Count()) == b'\0\0\0'
assert bytearray(b'a') * Reflected() == 42
assert Count() * bytearray(b'a') == b'aaa'
assert bytearray(b'a') * Count() == b'aaa'


class Items:
    def __init__(self):
        self.i = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.i += 1
        if self.i > 3:
            raise StopIteration
        return 256 if self.i == 2 else 65


for method in ['constructor', 'extend', 'slice']:
    it = Items()
    a = bytearray(b'x')
    try:
        if method == 'constructor':
            bytearray(it)
        elif method == 'extend':
            a.extend(it)
        else:
            a[:] = it
    except ValueError:
        pass
    else:
        assert False
    assert it.i == 2 and a == b'x'

for value in ['41 42', b'41 42', bytearray(b'41\t42')]:
    assert bytearray.fromhex(value) == b'AB'
    assert bytearray().fromhex(value) == b'AB'
for value in ['aa\u00a0bb', 'aa\x85bb', b'\xff']:
    try:
        bytearray.fromhex(value)
    except ValueError:
        pass
    else:
        assert False

for size in [-1, 0, 1, 4, 8]:
    a = bytearray(b'a\tb\n\tc\r\td')
    expected = (
        b'ab\nc\rd'
        if size <= 0
        else b'a' + b' ' * (size - 1 % size) + b'b\n' + b' ' * size + b'c\r' + b' ' * size + b'd'
    )
    assert a.expandtabs(tabsize=size) == expected
    assert isinstance(a.expandtabs(size), bytearray)

table = bytearray.maketrans(b'ab', bytearray(b'xy'))
assert isinstance(table, bytes) and len(table) == 256
assert bytearray(b'aabbcc').translate(table) == b'xxyycc'
assert bytearray(b'aabbcc').translate(table, delete=b'b') == b'xxcc'
assert bytearray(b'aabbcc').translate(None, b'b') == b'aacc'
assert bytearray().maketrans(b'a', b'b') == bytearray.maketrans(b'a', b'b')
