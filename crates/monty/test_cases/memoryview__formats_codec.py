# /// monty
# # Monty's native numeric layout is little-endian LP64, including on Windows.
# skip-cpython-windows = true
# ///

import math


class Indexed:
    def __index__(self):
        return 7


class Floating:
    def __float__(self):
        return 2.5


class Truth:
    def __bool__(self):
        return True


formats = [
    ('B', 1),
    ('b', 1),
    ('c', 1),
    ('?', 1),
    ('h', 2),
    ('H', 2),
    ('i', 4),
    ('I', 4),
    ('l', 8),
    ('L', 8),
    ('q', 8),
    ('Q', 8),
    ('n', 8),
    ('N', 8),
    ('e', 2),
    ('f', 4),
    ('d', 8),
    ('P', 8),
]
for code, size in formats:
    for prefix in ['', '@']:
        data = bytearray(size * 4)
        view = memoryview(data).cast(prefix + code)
        assert view.itemsize == size
        assert view.format == prefix + code
        assert len(view) == 4
        assert view.nbytes == size * 4
        assert view.shape == (4,)
        assert view.strides == (size,)
        if code == 'c':
            assert view.tolist() == [b'\0'] * 4
            view[0] = b'x'
            assert view[0] == b'x' and data[0] == 120
        elif code == '?':
            assert view.tolist() == [False] * 4
            for value in [None, 0, '', [], False]:
                view[0] = value
                assert view[0] is False
                assert data[0] == 0
            for value in [1, 'yes', b'x', Indexed(), Truth()]:
                view[0] = value
                assert view[0] is True
                assert data[0] == 1
        elif code in 'efd':
            assert view.tolist() == [0.0] * 4
            view[0] = Floating()
            assert view[0] == 2.5
            view[1] = Indexed()
            assert view[1] == 7.0
            view[2] = -0.0
            assert math.copysign(1.0, view[2]) == -1.0
            view[3] = float('nan')
            assert math.isnan(view[3])
        else:
            bits = size * 8
            signed = code in 'bhilqn'
            minimum = -(2 ** (bits - 1)) if signed or code == 'P' else 0
            maximum = 2 ** (bits - 1) - 1 if signed else 2**bits - 1
            for value in [minimum, 0, maximum]:
                view[0] = value
                expected = value % (2**bits) if code == 'P' else value
                assert view[0] == expected, (code, value, view[0])
                encoded = bytearray([(value >> (8 * i)) & 255 for i in range(size)])
                assert data[:size] == encoded
            for value in [minimum - 1, maximum + 1]:
                try:
                    view[0] = value
                except ValueError as exc:
                    assert str(exc) == "memoryview: invalid value for format '" + code + "'"
                else:
                    assert False, (code, value)
            view[1] = True
            assert view[1] == 1
            if code != 'P':
                view[2] = Indexed()
                assert view[2] == 7

for code, expected in [
    ('e', bytearray([0, 62])),
    ('f', bytearray([0, 0, 192, 63])),
    ('d', bytearray([0, 0, 0, 0, 0, 0, 248, 63])),
]:
    data = bytearray(len(expected))
    view = memoryview(data).cast(code)
    view[0] = 1.5
    assert data == expected

for code in ['L', 'Q', 'N', 'P']:
    assert memoryview(bytearray([255] * 8)).cast(code)[0] == 2**64 - 1

half = memoryview(bytearray(2)).cast('e')
half[0] = 1.1
assert half[0] == 1.099609375
single = memoryview(bytearray(4)).cast('f')
single[0] = 1.1
assert single[0] == 1.100000023841858
single[0] = 1e100
assert math.isinf(single[0])

for code in ['e', 'f', 'd']:
    view = memoryview(bytearray(8)).cast(code)
    view[0] = float('inf')
    assert math.isinf(view[0])
    view[0] = float('nan')
    assert not (view == view)
    other = memoryview(view.obj).cast(code)
    assert not (view == other)
    assert view.count(float('nan')) == 0
    try:
        view.index(float('nan'))
    except ValueError as exc:
        assert str(exc) == 'memoryview.index(x): x not found'
    else:
        assert False
    view[0] = -0.0
    other_data = bytearray(8)
    other_zero = memoryview(other_data).cast(code)
    assert view == other_zero
    try:
        view[0] = 2**1024
    except ValueError as exc:
        assert str(exc) == "memoryview: invalid value for format '" + code + "'"
    else:
        assert False

for value in [65520.0, -65520.0, 1e100]:
    try:
        half[0] = value
    except ValueError as exc:
        assert str(exc) == "memoryview: invalid value for format 'e'"
    else:
        assert False

for code in ['F', 'D', 'x', 'w', 'u', '<i', '>i', '=i', '2i', '@@i']:
    try:
        memoryview(bytearray(8)).cast(code)
    except ValueError as exc:
        assert (
            str(exc)
            == "memoryview: destination format must be a native single character format prefixed with an optional '@'"
        )
    else:
        assert False, code
