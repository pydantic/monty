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


# === A count with `__index__` repeats the sequence in either operand order ===
class Count:
    def __init__(self, n):
        self.n = n

    def __index__(self):
        return self.n


for sequence, _ in sequences:
    assert sequence * Count(2) == sequence * 2
    assert Count(2) * sequence == sequence * 2
    assert sequence * Count(-1) == sequence * 0
    repeated = sequence * 1
    repeated *= Count(3)
    assert repeated == sequence * 3


class RaisingCount:
    def __index__(self):
        raise ValueError('no count')


try:
    [1] * RaisingCount()
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'no count'
assert multiply_error('ab', Count('2')) == '__index__ returned non-int (type str)'
assert multiply_error(Count(None), (1,)) == '__index__ returned non-int (type NoneType)'


# === A count too large for an index names the count's type ===
def overflow_error(left, right):
    try:
        left * right
        assert False, 'expected multiplication to raise OverflowError'
    except OverflowError as exc:
        return str(exc)


for sequence, _ in sequences:
    assert overflow_error(sequence, Count(2**100)) == "cannot fit 'Count' into an index-sized integer"
    assert overflow_error(Count(2**100), sequence) == "cannot fit 'Count' into an index-sized integer"
    assert overflow_error(sequence, 2**100) == "cannot fit 'int' into an index-sized integer"
    # the check applies in both directions, from just past the index range
    assert overflow_error(sequence, Count(-(2**100))) == "cannot fit 'Count' into an index-sized integer"
    assert overflow_error(sequence, -(2**63) - 1) == "cannot fit 'int' into an index-sized integer"
    assert overflow_error(sequence, 2**63) == "cannot fit 'int' into an index-sized integer"
    assert sequence * -(2**31) == sequence * 0
repeated = [1]
try:
    repeated *= Count(2**100)
    assert False, 'expected augmented multiplication to raise OverflowError'
except OverflowError as exc:
    assert str(exc) == "cannot fit 'Count' into an index-sized integer"
