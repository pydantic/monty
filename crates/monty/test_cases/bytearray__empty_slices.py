# Adapted from CPython 3.14 Lib/test/test_bytes.py, ByteArrayTest.test_extended_set_del_slice.
# https://github.com/python/cpython/blob/3.14/Lib/test/test_bytes.py
indices = [None, 0, 1, 3, 19, 300, 1 << 333, -1, -2, -31, -300, -(1 << 333)]
source = list(range(32))
for start in indices:
    for stop in indices:
        for step in indices:
            if step == 0:
                continue
            key = slice(start, stop, step)
            selected = list(range(len(source)))[key]
            expected = bytearray([value for i, value in enumerate(source) if i not in selected])
            for replacement in [b'', bytearray(), []]:
                value = bytearray(source)
                alias = value
                value[key] = replacement
                assert value == expected, (start, stop, step, value)
                assert value is alias

value = bytearray(b'abc')
try:
    value[::2] = b'x'
except ValueError:
    pass
else:
    assert False
assert value == b'abc'
