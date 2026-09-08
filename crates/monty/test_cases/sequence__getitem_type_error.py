# Subscripting a sequence with a non-integer key: CPython words the TypeError
# per type, with `or slices` for the slice-able types and quotes only for str.
from collections import deque, namedtuple

P = namedtuple('P', 'a')


def key_error(obj, key):
    try:
        obj[key]
    except TypeError as exc:
        return str(exc)
    return None


assert key_error([1], 'k') == 'list indices must be integers or slices, not str'
assert key_error([1], 1.5) == 'list indices must be integers or slices, not float'
assert key_error([1], None) == 'list indices must be integers or slices, not NoneType'
assert key_error((1,), 'k') == 'tuple indices must be integers or slices, not str'
assert key_error(P(1), 'k') == 'tuple indices must be integers or slices, not str'
assert key_error(range(3), 'k') == 'range indices must be integers or slices, not str'
assert key_error(b'ab', 'k') == 'byte indices must be integers or slices, not str'
assert key_error(b'ab', 1.5) == 'byte indices must be integers or slices, not float'
assert key_error('ab', 'k') == "string indices must be integers, not 'str'"
assert key_error('ab', 1.5) == "string indices must be integers, not 'float'"
assert key_error(deque([1]), 'k') == "sequence index must be integer, not 'str'"

# assignment shares the list wording
items = [1]
try:
    items['k'] = 2
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'list indices must be integers or slices, not str'
