# === Basic list comprehension ===
assert [x for x in [1, 2, 3]] == [1, 2, 3]
assert [x * 2 for x in [1, 2, 3]] == [2, 4, 6]
assert [x + 1 for x in range(5)] == [1, 2, 3, 4, 5]

# === With filter ===
assert [x for x in [1, 2, 3, 4] if x > 2] == [3, 4]
assert [x for x in [1, 2, 3, 4, 5] if x % 2 == 0] == [2, 4]
assert [x for x in range(20) if x % 2 == 0 if x % 3 == 0] == [0, 6, 12, 18]
assert [x * 2 for x in [1, 2, 3, 4] if x > 1 if x < 4] == [4, 6]

# === Nested for ===
assert [x + y for x in [1, 2] for y in [10, 20]] == [11, 21, 12, 22]
assert [(x, y) for x in [1, 2] for y in ['a', 'b']] == [(1, 'a'), (1, 'b'), (2, 'a'), (2, 'b')]
assert [x * y for x in [1, 2, 3] for y in [10, 100]] == [10, 100, 20, 200, 30, 300]

# === Nested with filter ===
assert [x + y for x in [1, 2, 3] if x > 1 for y in [10, 20] if y > 10] == [22, 23]

# === Set comprehension ===
assert {x for x in [1, 2, 2, 3]} == {1, 2, 3}
assert {x for x in [1, 2, 3] if x > 1} == {2, 3}
assert {x * 2 for x in [1, 2, 3]} == {2, 4, 6}
assert {x % 3 for x in range(10)} == {0, 1, 2}

# === Dict comprehension ===
assert {x: x * 2 for x in [1, 2, 3]} == {1: 2, 2: 4, 3: 6}
assert {x: x for x in [1, 2, 3] if x > 1} == {2: 2, 3: 3}
assert {str(x): x for x in [1, 2, 3]} == {'1': 1, '2': 2, '3': 3}
assert {x: y for x in [1, 2] for y in [10, 20]} == {1: 20, 2: 20}

# === Scope isolation ===
x = 'outer'
result = [x for x in [1, 2, 3]]
assert x == 'outer'

y = 'before'
result2 = [y * 2 for y in [1, 2]]
assert y == 'before'

# === Access enclosing scope ===
multiplier = 10
assert [x * multiplier for x in [1, 2]] == [10, 20]

prefix = 'item_'
assert [prefix + str(x) for x in [1, 2, 3]] == ['item_1', 'item_2', 'item_3']

base = [1, 2, 3]
assert [x + 10 for x in base] == [11, 12, 13]


# === Capture when iter uses same name as target ===
def outer_capture_same_name():
    x = [1, 2, 3]

    def inner():
        return [x for x in x]

    return inner()


assert outer_capture_same_name() == [1, 2, 3]

# === Empty iterables ===
assert [x for x in []] == []
assert {x for x in []} == set()
assert {x: x for x in []} == {}

# === Filter removes all ===
assert [x for x in [1, 2, 3] if x > 10] == []
assert {x for x in [1, 2, 3] if x > 10} == set()
assert {x: x for x in [1, 2, 3] if x > 10} == {}

# === Complex expressions ===
assert [x**2 for x in [1, 2, 3, 4]] == [1, 4, 9, 16]
assert [len(s) for s in ['a', 'bb', 'ccc']] == [1, 2, 3]
assert [[y for y in range(x)] for x in [1, 2, 3]] == [[0], [0, 1], [0, 1, 2]]

# === Nested generator referencing prior loop var ===
# Second generator's iter references first generator's loop variable
assert [y for x in [[1, 2], [3, 4]] for y in x] == [1, 2, 3, 4]
assert [(x, y) for x in [1, 2] for y in range(x)] == [(1, 0), (2, 0), (2, 1)]


def outer_nested_comp():
    xs = [[1, 2], [3, 4]]

    def inner():
        return [y for x in xs for y in x]

    return inner()


assert outer_nested_comp() == [1, 2, 3, 4]

# === Tuple unpacking in comprehensions ===
pairs = [(1, 'a'), (2, 'b'), (3, 'c')]
assert [x for x, y in pairs] == [1, 2, 3]
assert [y for x, y in pairs] == ['a', 'b', 'c']
assert [str(x) + str(y) for x, y in [(1, 2), (3, 4)]] == ['12', '34']
assert [(y, x) for x, y in pairs] == [('a', 1), ('b', 2), ('c', 3)]

# Tuple unpacking with filter
assert [x for x, y in pairs if x > 1] == [2, 3]
assert [y for x, y in pairs if y != 'b'] == ['a', 'c']

# Triple unpacking
triples = [(1, 2, 3), (4, 5, 6)]
assert [a + b + c for a, b, c in triples] == [6, 15]
assert [b for a, b, c in triples] == [2, 5]

# Dict comprehension with unpacking
d = {k: v for k, v in pairs}
assert d == {1: 'a', 2: 'b', 3: 'c'}
assert {v: k for k, v in pairs} == {'a': 1, 'b': 2, 'c': 3}

# Set comprehension with unpacking
assert {x for x, y in pairs} == {1, 2, 3}
assert {y for x, y in pairs} == {'a', 'b', 'c'}

# Unpacking with dict.items()
d2 = {'x': 10, 'y': 20, 'z': 30}
assert [k for k, v in d2.items()] == ['x', 'y', 'z']
assert [v for k, v in d2.items()] == [10, 20, 30]
assert {v: k for k, v in d2.items()} == {10: 'x', 20: 'y', 30: 'z'}

# Nested comprehension with unpacking
matrix = [[(1, 2), (3, 4)], [(5, 6), (7, 8)]]
assert [[a + b for a, b in row] for row in matrix] == [[3, 7], [11, 15]]

# Scope isolation with unpacking (vars don't leak)
x = 'outer_x'
y = 'outer_y'
result = [x + y for x, y in [(1, 2)]]
assert x == 'outer_x'
assert y == 'outer_y'


# Unpacking in closure
def outer_unpack():
    items = [(1, 2), (3, 4)]

    def inner():
        return [a * b for a, b in items]

    return inner()


assert outer_unpack() == [2, 12]


# Capture variable used in unpacking pattern
def outer_shadow_unpack():
    x = 100

    def inner():
        # x in unpacking shadows the outer x, but we can still reference outer x in expression
        # Actually, the x in the comprehension shadows outer x, so this tests scope isolation
        pairs = [(1, 2), (3, 4)]
        return [x + y for x, y in pairs]

    return inner()


assert outer_shadow_unpack() == [3, 7]

# === Generator expressions (temporary: treated as list comprehensions) ===
# TODO: When proper generators are implemented, these should return generator objects
# instead of lists. For now, generator expressions are parsed as list comprehensions.
# See iter__generator_expr.py for tests, and iter__generator_expr_type.py for
# a type check test (xfail=cpython since CPython has real generators).

# Generator in list() call - works identically in both Monty and CPython
assert list(x for x in [1, 2, 3]) == [1, 2, 3]
assert tuple(x for x in [1, 2, 3]) == (1, 2, 3)

# Generator with condition
assert list(x for x in range(10) if x % 2 == 0) == [0, 2, 4, 6, 8]

# Nested generators
assert list(x + y for x in range(3) for y in range(2)) == [0, 1, 1, 2, 2, 3]

# Generator in sum()
assert sum(x for x in range(5)) == 10

# Generator with unpacking
pairs = [(1, 2), (3, 4)]
assert list(a + b for a, b in pairs) == [3, 7]

# list of strings join
assert ''.join(str(x) for x in range(5)) == '01234'
a = '1', '2', '3'
assert ''.join(a) == '123'

# === Regression: Iterator panic with try/except inside loop ===
# Issue: https://github.com/pydantic/monty/issues/177
# Verifies that exception handling in a comprehension inside a loop doesn't
# corrupt the outer loop's iterator (causing "expected Iterator on heap" panic).
# A prior loop is needed to potentially trigger incorrect stack depth tracking.
for _ in range(1):
    pass

for s in ['hello']:
    try:
        # Inner comprehension raises exception
        [int(c) for c in s]
    except ValueError:
        pass
