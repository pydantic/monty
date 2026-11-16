# Adapted from CPython's BaseBytesTest.test_ord and test_reversed.
# https://github.com/python/cpython/blob/3.14/Lib/test/test_bytes.py
for factory in [bytes, bytearray]:
    for source, expected in [(b'\0', 0), (b'A', 65), (b'\x80', 128), (b'\xff', 255)]:
        assert ord(factory(source)) == expected
    for source in [b'', b'ab', b'\xc3\xa9']:
        try:
            ord(factory(source))
        except TypeError as exc:
            assert str(exc) == f'ord() expected a character, but string of length {len(source)} found'
        else:
            assert False
    for source in [b'', b'a', b'abcd', b'\0\x80\xff']:
        data = factory(source)
        assert list(reversed(data)) == list(source)[::-1]
        assert data == source

assert ord('é') == 233
assert ord('😀') == 128512
try:
    ord([65])
except TypeError:
    pass
else:
    assert False
