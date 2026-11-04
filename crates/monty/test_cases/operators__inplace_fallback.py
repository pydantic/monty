from collections import deque


def apply(target, source, operation):
    if operation == 'mul':
        target *= source
    elif operation == 'or':
        target |= source
    elif operation == 'and':
        target &= source
    elif operation == 'sub':
        target -= source
    elif operation == 'lshift':
        target <<= source
    elif operation == 'rshift':
        target >>= source
    else:
        target ^= source
    return target


# Immutable operands replace the assigned value through binary fallback.
for value in [3, 2**70, 3.5]:
    assert apply(value, 2, 'mul') == value * 2
for value in [3, 2**70]:
    for operation, expected in [('or', value | 2), ('and', value & 2), ('sub', value - 2), ('xor', value ^ 2)]:
        assert apply(value, 2, operation) == expected
for value in [3, 2**70]:
    assert apply(value, 2, 'lshift') == value << 2
    assert apply(value, 2, 'rshift') == value >> 2
for operation, expected in [('or', {1, 2, 3}), ('and', {2}), ('sub', {1}), ('xor', {1, 3})]:
    original = frozenset({1, 2})
    result = apply(original, {2, 3}, operation)
    assert isinstance(result, frozenset)
    assert result == expected and original == {1, 2}


class Count:
    def __index__(self):
        return 2


for value in [(1, 2), ''.join(['ab'] * 32), b''.join([b'ab'] * 32)]:
    alias = value
    result = apply(value, Count(), 'mul')
    assert result == value * 2 and result is not alias


class Product:
    def __mul__(self, other):
        return 17


assert apply(Product(), 2, 'mul') == 17


# Unsupported operands must reach the binary TypeError without mutating aliases.
for operation in ['or', 'and', 'sub', 'xor']:
    for source in [1, [2]]:
        target = {1, 2}
        alias = target
        try:
            apply(target, source, operation)
        except TypeError:
            pass
        else:
            assert False
        assert target is alias and target == {1, 2}

for target in [[1], deque([1])]:
    alias = target
    try:
        apply(target, 1.5, 'mul')
    except TypeError:
        pass
    else:
        assert False
    assert target is alias and list(target) == [1]

for target in [1, [1], (1,), ''.join(['a'] * 32), b''.join([b'a'] * 32)]:
    try:
        apply(target, [], 'xor')
    except TypeError:
        pass
    else:
        assert False
