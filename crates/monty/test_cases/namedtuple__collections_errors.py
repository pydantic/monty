from collections import namedtuple


def expect_error(callback, expected_type, expected_message):
    try:
        callback()
    except expected_type as exc:
        assert exc.args[0] == expected_message, f'expected {expected_message!r}, got {exc.args!r}'
    else:
        assert False, f'expected {expected_type.__name__}: {expected_message}'


# === Factory validation ===
expect_error(
    lambda: namedtuple('1Point', ['x']),
    ValueError,
    "Type names and field names must be valid identifiers: '1Point'",
)
expect_error(
    lambda: namedtuple('class', ['x']),
    ValueError,
    "Type names and field names cannot be a keyword: 'class'",
)
expect_error(
    lambda: namedtuple('Point', ['1x']),
    ValueError,
    "Type names and field names must be valid identifiers: '1x'",
)
expect_error(
    lambda: namedtuple('Point', ['class']),
    ValueError,
    "Type names and field names cannot be a keyword: 'class'",
)
expect_error(
    lambda: namedtuple('Point', ['_x']),
    ValueError,
    "Field names cannot start with an underscore: '_x'",
)
expect_error(
    lambda: namedtuple('Point', ['x', 'x']),
    ValueError,
    "Encountered duplicate field name: 'x'",
)
expect_error(
    lambda: namedtuple('Point', ['x'], defaults=[1, 2]),
    TypeError,
    'Got more default values than field names',
)

# === Constructor binding ===
Point = namedtuple('Point', ['x', 'y'])
expect_error(
    lambda: Point(1),
    TypeError,
    "Point.__new__() missing 1 required positional argument: 'y'",
)
expect_error(
    lambda: Point(1, 2, 3),
    TypeError,
    'Point.__new__() takes 3 positional arguments but 4 were given',
)
expect_error(
    lambda: Point(1, x=2),
    TypeError,
    "Point.__new__() got multiple values for argument 'x'",
)
expect_error(
    lambda: Point(z=1),
    TypeError,
    "Point.__new__() got an unexpected keyword argument 'z'",
)
