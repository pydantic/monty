class Count:
    def __index__(self):
        return 3


assert bytearray(Count()) == b'\0\0\0'
a = bytearray(b'ab')
b = a
a *= Count()
assert a is b and a == b'ababab'
a *= 0
assert a is b and a == b''
a += a
assert a is b

a.extend(b'ab')
try:
    a += a
except BufferError as exc:
    assert str(exc) == 'Existing exports of data: object cannot be re-sized'
else:
    assert False
assert a is b and a == b'ab'


class Reflected:
    def __rmul__(self, other):
        return 42


a *= Reflected()
assert a == 42 and b == b'ab'
assert bytearray(b'a') * Reflected() == 42
assert Count() * bytearray(b'a') == b'aaa'
assert bytearray(b'a') * Count() == b'aaa'


a = bytearray(b'ab')
alias = a
a += b'c'
assert a is alias and a == b'abc'
a *= 2
assert a is alias and a == b'abcabc'
