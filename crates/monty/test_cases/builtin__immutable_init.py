from collections import namedtuple

# Exact tuples retain their identity and shared children.
child = []
original_tuple = (child, 'a heap-backed tuple member', 42)
for _ in range(100):
    tuple_alias = tuple(original_tuple)
    assert tuple_alias is original_tuple
    assert tuple_alias[0] is child
child.append(7)
assert original_tuple[0] == [7]
empty_tuple = ()
assert tuple(empty_tuple) is empty_tuple

# Named tuples and mutable sequences still produce plain tuples.
Pair = namedtuple('Pair', ['left', 'right'])
pair = Pair(child, 2)
converted = tuple(pair)
assert type(converted) is tuple
assert converted is not pair
assert converted[0] is child
assert converted == (child, 2)
mutable = [1, 2]
converted = tuple(mutable)
mutable.append(3)
assert converted == (1, 2)

# Construction from a frozenset does not hash its elements again.
hash_calls = []


class Key:
    def __hash__(self):
        hash_calls.append(1)
        return 7


key = Key()
original_frozenset = frozenset([key])
hash_calls.clear()
for _ in range(100):
    frozen_alias = frozenset(original_frozenset)
    assert frozen_alias is original_frozenset
assert hash_calls == []
assert next(iter(frozen_alias)) is key
empty_frozenset = frozenset()
assert frozenset(empty_frozenset) is empty_frozenset
mutable_set = {1, 2}
converted_set = frozenset(mutable_set)
mutable_set.add(3)
assert converted_set == frozenset([1, 2])

# Interned and heap bytes retain identity, including keyword construction.
for original_bytes in [b'', b'ab', b'abcdef' * 1000, bytes(128)]:
    for _ in range(100):
        assert bytes(original_bytes) is original_bytes
        assert bytes(source=original_bytes) is original_bytes

# Encoding/errors validation still precedes the identity shortcut.
for invalid in [
    lambda: bytes(b'ab', 'utf-8'),
    lambda: bytes(b'ab', encoding='utf-8'),
]:
    try:
        invalid()
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == 'encoding without a string argument'

try:
    bytes(b'ab', errors='strict')
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'errors without a string argument'

assert bytes('héllo', 'utf-8') == b'h\xc3\xa9llo'
assert bytes(3) == b'\x00\x00\x00'
