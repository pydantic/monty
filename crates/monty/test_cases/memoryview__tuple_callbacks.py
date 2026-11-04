calls = []
response = 0


class Coordinate:
    def __init__(self, axis):
        self.axis = axis

    def __index__(self):
        calls.append(self.axis)
        return response


def access(key, write):
    if write:
        view[key] = 1
    else:
        return view[key]


view = memoryview(bytearray(8)).cast('B', [2, 2, 2])
for write in [False, True]:
    for key, kind, message in [
        ((Coordinate(0),), NotImplementedError, 'sub-views are not implemented'),
        ((Coordinate(0), 0, 0, 0), TypeError, 'cannot index 3-dimension view with 4-element tuple'),
        ((Coordinate(0), 'x', 0), TypeError, 'memoryview: invalid slice key'),
    ]:
        calls.clear()
        try:
            access(key, write)
        except Exception as exc:
            assert isinstance(exc, kind)
            assert str(exc) == message
        else:
            assert False
        assert calls == []

    response = 2
    calls.clear()
    try:
        access((Coordinate(0), Coordinate(1), 0), write)
    except IndexError as exc:
        assert str(exc) == 'index out of bounds on dimension 1'
    else:
        assert False
    assert calls == [0]

    response = 2**100
    calls.clear()
    try:
        access((Coordinate(0), 0, 0), write)
    except IndexError as exc:
        assert str(exc) == "cannot fit 'Coordinate' into an index-sized integer"
    else:
        assert False
    assert calls == [0]

# Multidimensional scalar assignment rejects the operation before converting the key.
for response in [0, 2**100, None]:
    calls.clear()
    try:
        view[Coordinate(0)] = 1
    except NotImplementedError as exc:
        assert str(exc) == 'sub-views are not implemented'
    else:
        assert False
    assert calls == []

for write in [False, True]:
    calls.clear()
    response = 2**100
    one = memoryview(bytearray(8))
    try:
        if write:
            one[Coordinate(0)] = 1
        else:
            one[Coordinate(0)]
    except IndexError as exc:
        assert str(exc) == "cannot fit 'Coordinate' into an index-sized integer"
    else:
        assert False
    assert calls == [0]
