owner = bytearray(b'abc')
view = memoryview(owner)
separator = bytes(bytearray(b':'))
view.release()

try:
    view.hex(separator)
except ValueError as exc:
    assert str(exc) == 'operation forbidden on released memoryview object'
else:
    assert False

owner.append(0)
assert owner == b'abc\0'
