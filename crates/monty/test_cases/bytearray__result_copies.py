# Adapted from CPython 3.14 Lib/test/test_bytes.py, BytearrayPEP3137Test.test_returns_new_copy.
# https://github.com/python/cpython/blob/3.14/Lib/test/test_bytes.py
value = bytearray(b'1234')
results = [
    value.zfill(3),
    value.rjust(3),
    value.ljust(3),
    value.center(3),
    value.split()[0],
    value.rsplit()[0],
    value.partition(b'.')[0],
    value.rpartition(b'.')[2],
    value.splitlines()[0],
    value.replace(b'', b''),
    bytearray().join([value]),
    value[:],
]
for result in results:
    assert type(result) is bytearray
    assert result == value
    assert result is not value
    result[0] = 88
    assert value == b'1234'
