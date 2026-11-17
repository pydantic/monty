# /// monty
# mount-fs = true
# ///
path = root / 'bytearray.bin'
data = bytearray(b'abc')
assert path.write_bytes(data) == 3
data[0] = 120
assert path.read_bytes() == b'abc'

with path.open('w+b') as f:
    assert f.write(data) == 3
    data[1] = 121
    for part in [data, bytearray(b'!'), b'?']:
        assert f.write(part) == len(part)
    assert f.tell() == 8
    f.seek(0)
    assert f.read() == b'xbcxyc!?'

with path.open('w') as f:
    try:
        f.write(data)
    except TypeError:
        pass
    else:
        assert False
