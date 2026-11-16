# Adapted from CPython's BaseBytesTest.test_hex_separator_basics.
# https://github.com/python/cpython/blob/3.14/Lib/test/test_bytes.py


class GroupSize:
    def __index__(self):
        return -2


for factory in [bytes, bytearray]:
    for source in [b'', b'ab', b'abc']:
        data = factory(source)
        assert data.hex() == source.hex()
        assert data.hex(':', 0) == data.hex()
        assert data.hex(b':', 1) == ':'.join(source[i : i + 1].hex() for i in range(len(source)))
        for args in [(None,), (None, 0), (None, 2), (':', None), (b':', None), (bytearray(b':'), 1)]:
            try:
                data.hex(*args)
            except TypeError:
                pass
            else:
                assert False, args
    assert factory(b'abc').hex(':', GroupSize()) == '6162:63'
