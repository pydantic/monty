# Adapted from CPython's BaseBytesTest.test_none_arguments and prefix/suffix tests.
# https://github.com/python/cpython/blob/3.14/Lib/test/test_bytes.py
for factory in [bytes, bytearray]:
    data = factory(b'abcabc')
    assert data.startswith(bytearray(b'ab'))
    assert data.endswith(bytearray(b'bc'))
    assert not data.startswith(bytearray(b'bc'))
    assert not data.endswith(bytearray(b'ab'))
    assert data.startswith((b'x', bytearray(b'bc')), 1, 3)
    assert data.endswith((bytearray(b'x'), b'ab'), -6, -4)
    assert data.startswith(bytearray(), None, None)
    assert data.endswith(bytearray(), 6)
    assert not data.startswith(bytearray(), 7)
    assert not data.endswith(bytearray(), 2, 1)
    assert not data.startswith(())
    assert not data.endswith(())
    # A matching entry prevents inspection of later invalid tuple entries.
    assert data.startswith((bytearray(b'a'), None))
    assert data.endswith((bytearray(b'c'), 42))
    for candidate in [None, 97, 'a', (None, b'a'), (b'x', 97), (b'x', (b'a',))]:
        for end in [0, 6]:
            try:
                data.startswith(candidate, 0, end)
            except TypeError:
                pass
            else:
                assert False
            try:
                data.endswith(candidate, 0, end)
            except TypeError:
                pass
            else:
                assert False

data = bytearray(b'ab')
prefix = bytearray(b'x')


class Bound:
    def __index__(self):
        data.extend(b'cd')
        prefix[:] = b'cd'
        return 2


assert data.startswith(prefix, Bound())
assert data.endswith((b'x', prefix))
assert data.startswith(data)
assert data.endswith((data, None))
