from collections import deque, namedtuple

Point = namedtuple('Point', ['x'])


class BadCount:
    pass


def multiply_error(left, right):
    """Capture exact multiplication errors across operand orders and storage representations."""
    try:
        left * right
        assert False, 'expected multiplication to raise TypeError'
    except TypeError as exc:
        return str(exc)


sequences = [
    ('ab', 'str'),
    (''.join(['a', 'b']), 'str'),
    (b'ab', 'bytes'),
    (b''.join([b'a', b'b']), 'bytes'),
    ([1], 'list'),
    ((1,), 'tuple'),
    (Point(1), 'Point'),
    (deque([1]), 'collections.deque'),
    ('', 'str'),
    (b'', 'bytes'),
    ([], 'list'),
    ((), 'tuple'),
]

# === Invalid counts in either operand order, including augmented assignment ===
for sequence, _ in sequences:
    for count, name in [
        (1.5, 'float'),
        (None, 'NoneType'),
        ({}, 'dict'),
        (range(2), 'range'),
        (1j, 'complex'),
        (BadCount(), 'BadCount'),
    ]:
        expected = f"can't multiply sequence by non-int of type '{name}'"
        assert multiply_error(sequence, count) == expected
        assert multiply_error(count, sequence) == expected
        try:
            sequence *= count
            assert False, 'expected augmented multiplication to raise TypeError'
        except TypeError as exc:
            assert str(exc) == expected

# === When both operands are sequences, the right operand is the invalid count ===
for left, _ in sequences:
    for right, right_name in sequences:
        assert multiply_error(left, right) == f"can't multiply sequence by non-int of type '{right_name}'"

# === Valid repetition still succeeds through direct and reflected dispatch ===
for sequence, _ in sequences:
    assert sequence * 2 == 2 * sequence
    assert sequence * True == sequence
    assert True * sequence == sequence

# === Non-repeatable operands retain the generic multiplication error ===
for value, name in [(range(2), 'range'), ({}, 'dict'), (BadCount(), 'BadCount')]:
    assert multiply_error(value, 1.5) == f"unsupported operand type(s) for *: '{name}' and 'float'"
    assert multiply_error(1.5, value) == f"unsupported operand type(s) for *: 'float' and '{name}'"
