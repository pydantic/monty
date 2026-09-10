import itertools

# === Boolean 'and' operator ===
# returns first falsy value, or last value if all truthy
assert (5 and 3) == 3
assert (0 and 3) == 0
assert (1 and 2 and 3) == 3

# === Boolean 'or' operator ===
# returns first truthy value, or last value if all falsy
assert (5 or 3) == 5
assert (0 or 3) == 3
assert (0 or 0 or 3) == 3

# === Boolean 'not' operator ===
assert (not 5) == False
assert (not 0) == True
assert (not None) == True

# === Complex boolean expressions ===
assert ((1 and 2) or (3 and 0)) == 2
assert (not (0 and 1)) == True

# === Boolean bitwise operators ===
assert (True & True) == True
assert (True & False) == False
assert (False | True) == True
assert (False | False) == False
assert (True ^ False) == True
assert (True ^ True) == False
assert type(True & True) == bool
assert type(False | True) == bool
assert type(True ^ False) == bool

# Mixing bool and int uses integer bitwise operations.
assert type(True & 1) == int
assert type(1 | False) == int
assert type(True ^ 1) == int

# === bool is accepted wherever an integer argument is ===
# `bool` is an `int` subclass in CPython, so every "cannot be interpreted as an
# integer" consumer takes it directly.
assert list(range(True)) == [0]
assert list(range(False, True)) == [0]
assert list(range(False, True, True)) == [0]
assert 'x'.center(True) == 'x'
assert 'x'.ljust(True) == 'x'
assert 'x'.rjust(True) == 'x'
assert 'abcabc'.find('b', True) == 1
assert b'abcabc'.find(b'b', True) == 1
assert 'a\tb'.expandtabs(True) == 'a b'
assert list(itertools.repeat('x', True)) == ['x']
assert round(1.234, True) == 1.2

# The same holds for indexing and slicing, which take a separate path.
assert [10, 20, 30][True] == 20
assert [10, 20, 30][False] == 10
assert [1, 2, 3, 4][True:] == [2, 3, 4]
assert [1, 2, 3, 4][:True] == [1]
