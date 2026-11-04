def expect_error(function, kind, message):
    try:
        function()
    except Exception as exc:
        assert isinstance(exc, kind), (type(exc).__name__, kind.__name__, str(exc))
        assert str(exc) == message, (str(exc), message)
    else:
        assert False, message


def assign(view, key, value):
    view[key] = value


base = memoryview(bytearray(range(24)))
matrix = base.cast('B', [2, 3, 4])
zero = base[:1].cast('B', [])
for fmt in ['', '@', '@@I', '<I', '>I', '=I', 'F', 'D', 'x', '2I']:
    expect_error(
        lambda: base.cast(fmt),
        ValueError,
        "memoryview: destination format must be a native single character format prefixed with an optional '@'",
    )
for shape in [None, '24', 24, iter([24])]:
    expect_error(lambda: base.cast('B', shape), TypeError, 'shape must be a list or a tuple')
for shape in [[0], [-1]]:
    expect_error(lambda: base.cast('B', shape), ValueError, 'memoryview.cast(): elements of shape must be integers > 0')
expect_error(lambda: base.cast('B', [2.0, 12]), TypeError, 'memoryview.cast(): elements of shape must be integers')
expect_error(lambda: base.cast('B', [2**100]), OverflowError, 'Python int too large to convert to C ssize_t')
expect_error(lambda: base.cast('B', [2**62, 4]), ValueError, 'memoryview.cast(): product(shape) > SSIZE_MAX')
expect_error(lambda: base.cast('B', [1] * 65), ValueError, 'memoryview: number of dimensions must not exceed 64')
expect_error(lambda: base.cast('B', [2, 2]), TypeError, 'memoryview: product(shape) * itemsize != buffer size')
expect_error(lambda: base[:3].cast('I'), TypeError, 'memoryview: length is not a multiple of itemsize')
expect_error(lambda: base[::2].cast('B'), TypeError, 'memoryview: casts are restricted to C-contiguous views')
expect_error(lambda: base.cast('I').cast('H'), TypeError, 'memoryview: cannot cast between two non-byte formats')
expect_error(lambda: matrix.cast('B', [2, 12]), TypeError, 'memoryview: cast must be 1D -> ND or ND -> 1D')
expect_error(lambda: zero.cast('B', []), TypeError, 'memoryview: cast must be 1D -> ND or ND -> 1D')
for shape in [[], [0], [-1], [1] * 65]:
    expect_error(
        lambda: base[:0].cast('B', shape), TypeError, 'memoryview: cannot cast view with zeros in shape or strides'
    )
for key in [1, 99]:
    expect_error(lambda: matrix[key], NotImplementedError, 'multi-dimensional sub-views are not implemented')
    expect_error(lambda: assign(matrix, key, 1), NotImplementedError, 'sub-views are not implemented')
for key in [(), (1,), (2**100,)]:
    expect_error(lambda: matrix[key], NotImplementedError, 'sub-views are not implemented')
expect_error(lambda: matrix[1, 2, 3, 4], TypeError, 'cannot index 3-dimension view with 4-element tuple')
expect_error(lambda: matrix[0, 0, 4], IndexError, 'index out of bounds on dimension 3')
expect_error(lambda: matrix[1.0, 0, 0], TypeError, 'memoryview: invalid slice key')
expect_error(lambda: matrix[:, :, :], NotImplementedError, 'multi-dimensional slicing is not implemented')
expect_error(lambda: matrix[0, :, :], TypeError, 'memoryview: invalid slice key')
expect_error(
    lambda: assign(matrix, slice(None), matrix),
    NotImplementedError,
    'memoryview slice assignments are currently restricted to ndim = 1',
)
expect_error(lambda: next(iter(matrix)), NotImplementedError, 'multi-dimensional sub-views are not implemented')
expect_error(lambda: matrix.count(0), NotImplementedError, 'multi-dimensional sub-views are not implemented')
expect_error(lambda: matrix.index(0), NotImplementedError, 'multi-dimensional lookup is not implemented')
for action in [lambda: zero[0], lambda: zero[:], lambda: iter(zero), lambda: zero.count(0)]:
    expect_error(action, TypeError, 'invalid indexing of 0-dim memory')
expect_error(lambda: zero.index(0), TypeError, 'invalid lookup on 0-dim memory')
expect_error(lambda: len(zero), TypeError, '0-dim memory has no length')
expect_error(lambda: bool(zero), TypeError, '0-dim memory has no length')
expect_error(
    lambda: hash(base.cast('I').toreadonly()),
    ValueError,
    "memoryview: hashing is restricted to formats 'B', 'b' or 'c'",
)

# Layout and shape admission precede parsing the destination format.
for view in [base[::2], base[:0][::2]]:
    for shape in [None, [], [1] * 65]:
        expect_error(lambda: view.cast('', shape), TypeError, 'memoryview: casts are restricted to C-contiguous views')
for view in [base[:0], matrix[:0]]:
    expect_error(lambda: view.cast('', None), TypeError, 'memoryview: cannot cast view with zeros in shape or strides')
expect_error(lambda: base.cast('', None), TypeError, 'shape must be a list or a tuple')
expect_error(lambda: base.cast('', [1] * 65), ValueError, 'memoryview: number of dimensions must not exceed 64')
expect_error(lambda: matrix.cast('', []), TypeError, 'memoryview: cast must be 1D -> ND or ND -> 1D')
expect_error(lambda: matrix.cast('H', []), TypeError, 'memoryview: cast must be 1D -> ND or ND -> 1D')

# Creating an iterator fails immediately, before a wrapper can retain the view.
for view in [matrix, matrix[:0], matrix[:1], matrix.toreadonly()]:
    expect_error(lambda: iter(view), NotImplementedError, 'multi-dimensional sub-views are not implemented')
