# === sum() ===
# Basic sum operations
assert sum([1, 2, 3]) == 6
assert sum([1, 2, 3], 10) == 16
assert sum(()) == 0
assert sum([], 5) == 5
assert sum(range(5)) == 10
assert sum([1.5, 2.5, 3.0], 0.0) == 7.0
# Note: sum of floats without start requires py_add to support int+float

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
