# === sum() ===
# Basic sum operations
assert sum([1, 2, 3]) == 6
assert sum([1, 2, 3], 10) == 16
assert sum([1, 2, 3], start=10) == 16
assert sum(()) == 0
assert sum([], 5) == 5
assert sum(range(5)) == 10
assert sum([1.5, 2.5, 3.0], 0.0) == 7.0
assert sum([2**62, 2**62, 1]) == 2**63 + 1

# floats are added with compensated summation, as in CPython 3.12+, so rounding errors
# don't build up the way they do in a plain left-to-right loop
assert sum([0.1, 0.2, 0.3]) == 0.6
assert sum([0.1] * 10) == 1.0
assert sum([0.1] * 10, 0.0) == 1.0
assert sum([1e100, 1.0, -1e100]) == 1.0
assert sum([1.0, 1e16, -1e16, 1.0]) == 2.0
assert sum([0.1, 1, 0.2, True, 0.3]) == 2.6
assert sum([0.5, 2**64, -(2**64)]) == 0.5
# complex totals are compensated per part, and a real item only changes the real part
assert sum([0.1j] * 10) == 1j
assert sum([1j, 0.1, 0.2, 0.3]) == 0.6 + 1j
assert sum([0.1, 0.2, 0.3], 0j) == 0.6 + 0j
assert sum([0.5j, 2**64, 0.5, -(2**64)]) == 0.5 + 0.5j
assert repr(sum([1.0], complex(0.0, -0.0))) == '(1-0j)'
# the compensation is left out where it would drop the sign of a zero or turn inf into nan
assert repr(sum([-0.0], -0.0)) == '-0.0'
assert sum([1e308, 1e308]) == float('inf')
# CPython's fast loops only run while the total is an int, float or complex, so after a
# bool start or an int total past 64 bits the rest is added with plain `+`
assert sum([1e100, 1.0, -1e100], True) == 0.0
assert sum([2**63, -(2**63), 0.1, 0.2, 0.3]) == 0.6000000000000001
# and a float total is rounded when a complex item turns it complex, dropping its compensation
assert sum([0.1, 0.2, 1j, 0.3]) == 0.6000000000000001 + 1j
# items that can't join a float or complex total fail as they would with `+`
try:
    sum([0.5, 10**400])
    assert False, 'expected OverflowError'
except OverflowError as e:
    assert str(e) == 'int too large to convert to float'
try:
    sum([1j, 'x'])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "unsupported operand type(s) for +: 'complex' and 'str'"

# str/bytes start values are rejected, pointing at join() instead
try:
    sum([], 'x')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "sum() can't sum strings [use ''.join(seq) instead]"

try:
    sum([], b'x')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "sum() can't sum bytes [use b''.join(seq) instead]"

try:
    sum([b'a'], start=b'')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "sum() can't sum bytes [use b''.join(seq) instead]"

# sum with different iterables
assert sum({1, 2, 3}) == 6
assert sum({1: 'a', 2: 'b', 3: 'c'}) == 6

# === any() ===
# Basic any operations
assert any([True, False, False]) == True
assert any([False, False, False]) == False
assert any([]) == False
assert any([0, 0, 1]) == True
assert any([0, '', None]) == False
assert any(['', 'hello']) == True
assert any(range(0, 5)) == True
assert any(range(0, 1)) == False

# === all() ===
# Basic all operations
assert all([True, True, True]) == True
assert all([True, False, True]) == False
assert all([]) == True
assert all([1, 2, 3]) == True
assert all([1, 0, 3]) == False
assert all(['a', 'b', 'c']) == True
assert all(['a', '', 'c']) == False

# More edge cases with nested structures
assert any([[1], [], [3]]) == True
assert all([[1], [2], [3]]) == True

# sum with lists (list + list is supported)
assert sum([[1], [2], [3]], []) == [1, 2, 3]
# Note: sum with tuples requires Tuple py_add which is not implemented
