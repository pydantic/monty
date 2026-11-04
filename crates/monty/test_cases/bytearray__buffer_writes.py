# /// monty
# mount-fs = true
# ///

path = root / 'buffer_writes.bin'
assert path.write_bytes(bytearray(b'ab')) == 2
assert path.read_bytes() == b'ab'
assert path.write_bytes(memoryview(bytearray(b'cd'))) == 2
assert path.read_bytes() == b'cd'

with open(path, 'wb') as file:
    assert file.write(bytearray(b'ef')) == 2
with open(path, 'ab') as file:
    assert file.write(memoryview(bytearray(b'gh'))) == 2
assert path.read_bytes() == b'efgh'

view = memoryview(b'123')[::2]
with open(path, 'wb') as file:
    try:
        file.write(view)
    except BufferError:
        pass
    else:
        assert False

view.release()
try:
    path.write_bytes(view)
except ValueError as exc:
    assert str(exc) == 'operation forbidden on released memoryview object'
else:
    assert False
