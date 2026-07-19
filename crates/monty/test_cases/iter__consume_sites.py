# Consumption sites that drive an existing iterator: `in`, PEP 448 `*` unpack,
# and sequence unpacking. Each has its own iterable dispatch, so each needs
# wiring to the iterator protocol separately.

# === `in` consumes the iterator ===
assert 2 in iter([1, 2, 3])
assert 9 not in iter([1, 2, 3])
it = iter([1, 2, 3])
assert 2 in it
assert list(it) == [3]

# === PEP 448 `*` unpack ===
assert [*iter([1, 2]), 9] == [1, 2, 9]
assert (*iter([1, 2]),) == (1, 2)
assert {*iter([1, 2])} == {1, 2}

# === sequence unpacking ===
a, b = iter([1, 2])
assert (a, b) == (1, 2)
a, *rest = iter([1, 2, 3])
assert (a, rest) == (1, [2, 3])

# === unpack size errors, and how much is consumed ===
try:
    a, b, c = iter([1, 2])
    assert False, 'expected ValueError for too few values'
except ValueError as e:
    assert str(e) == 'not enough values to unpack (expected 3, got 2)'

# An over-long iterator stops at the first surplus item rather than draining,
# so there is no total in the message and the rest stays available.
src = iter([1, 2, 3, 4, 5])
try:
    a, b = src
    assert False, 'expected ValueError for too many values'
except ValueError as e:
    assert str(e) == 'too many values to unpack (expected 2)'
assert list(src) == [4, 5]

# === reversed() is not implied by being iterable ===
# CPython needs __reversed__, or __len__ + __getitem__; a one-shot iterator and
# an unordered set have neither.
try:
    reversed(iter([1, 2]))
    assert False, 'expected TypeError for reversed(iterator)'
except TypeError as e:
    # Only the tail is asserted here: Monty names every iterator `iterator`
    # while CPython says `list_iterator` (a documented divergence), and this
    # file is dual-run against both engines.
    assert str(e).endswith('object is not reversible'), 'reversed(iterator) message'
try:
    reversed({1, 2})
    assert False, 'expected TypeError for reversed(set)'
except TypeError as e:
    assert str(e) == "'set' object is not reversible"
assert list(reversed([1, 2])) == [2, 1]
assert list(reversed(range(3))) == [2, 1, 0]
assert list(reversed({'a': 1, 'b': 2})) == ['b', 'a']
