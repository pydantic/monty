# === Basic list comprehension ===
assert [x for x in [1, 2, 3]] == [1, 2, 3], 'identity'
assert [x * 2 for x in [1, 2, 3]] == [2, 4, 6], 'transform'
assert [x + 1 for x in range(5)] == [1, 2, 3, 4, 5], 'range'

# === With filter ===
assert [x for x in [1, 2, 3, 4] if x > 2] == [3, 4], 'filter'
assert [x for x in [1, 2, 3, 4, 5] if x % 2 == 0] == [2, 4], 'even filter'
assert [x for x in range(20) if x % 2 == 0 if x % 3 == 0] == [0, 6, 12, 18], 'multi-filter'
assert [x * 2 for x in [1, 2, 3, 4] if x > 1 if x < 4] == [4, 6], 'transform with multi-filter'

# === Nested for ===
assert [x + y for x in [1, 2] for y in [10, 20]] == [11, 21, 12, 22], 'nested'
assert [(x, y) for x in [1, 2] for y in ['a', 'b']] == [(1, 'a'), (1, 'b'), (2, 'a'), (2, 'b')], 'nested tuple'
assert [x * y for x in [1, 2, 3] for y in [10, 100]] == [10, 100, 20, 200, 30, 300], 'nested multiply'

# === Nested with filter ===
assert [x + y for x in [1, 2, 3] if x > 1 for y in [10, 20] if y > 10] == [22, 23], 'nested with filters'

# === Set comprehension ===
assert {x for x in [1, 2, 2, 3]} == {1, 2, 3}, 'set dedup'
assert {x for x in [1, 2, 3] if x > 1} == {2, 3}, 'set filter'
assert {x * 2 for x in [1, 2, 3]} == {2, 4, 6}, 'set transform'
assert {x % 3 for x in range(10)} == {0, 1, 2}, 'set modulo'

# === Dict comprehension ===
assert {x: x * 2 for x in [1, 2, 3]} == {1: 2, 2: 4, 3: 6}, 'dict'
assert {x: x for x in [1, 2, 3] if x > 1} == {2: 2, 3: 3}, 'dict filter'
assert {str(x): x for x in [1, 2, 3]} == {'1': 1, '2': 2, '3': 3}, 'dict str keys'
assert {x: y for x in [1, 2] for y in [10, 20]} == {1: 20, 2: 20}, 'dict nested overwrites'

# === Scope isolation ===
x = 'outer'
result = [x for x in [1, 2, 3]]
assert x == 'outer', 'loop var does not leak'

y = 'before'
result2 = [y * 2 for y in [1, 2]]
assert y == 'before', 'loop var y does not leak'

# === Access enclosing scope ===
multiplier = 10
assert [x * multiplier for x in [1, 2]] == [10, 20], 'closure'

prefix = 'item_'
assert [prefix + str(x) for x in [1, 2, 3]] == ['item_1', 'item_2', 'item_3'], 'closure string'

base = [1, 2, 3]
assert [x + 10 for x in base] == [11, 12, 13], 'closure list'


# === Capture when iter uses same name as target ===
def outer_capture_same_name():
    x = [1, 2, 3]

    def inner():
        return [x for x in x]

    return inner()


assert outer_capture_same_name() == [1, 2, 3], 'iter uses outer x'

# === Empty iterables ===
assert [x for x in []] == [], 'empty list'
assert {x for x in []} == set(), 'empty set'
assert {x: x for x in []} == {}, 'empty dict'

# === Filter removes all ===
assert [x for x in [1, 2, 3] if x > 10] == [], 'filter all'
assert {x for x in [1, 2, 3] if x > 10} == set(), 'set filter all'
assert {x: x for x in [1, 2, 3] if x > 10} == {}, 'dict filter all'

# === Complex expressions ===
assert [x**2 for x in [1, 2, 3, 4]] == [1, 4, 9, 16], 'square'
assert [len(s) for s in ['a', 'bb', 'ccc']] == [1, 2, 3], 'len'
assert [[y for y in range(x)] for x in [1, 2, 3]] == [[0], [0, 1], [0, 1, 2]], 'nested comprehension'

# === Nested generator referencing prior loop var ===
# Second generator's iter references first generator's loop variable
assert [y for x in [[1, 2], [3, 4]] for y in x] == [1, 2, 3, 4], 'flatten nested lists'
assert [(x, y) for x in [1, 2] for y in range(x)] == [(1, 0), (2, 0), (2, 1)], 'second iter uses first var'


def outer_nested_comp():
    xs = [[1, 2], [3, 4]]

    def inner():
        return [y for x in xs for y in x]

    return inner()


assert outer_nested_comp() == [1, 2, 3, 4], 'nested comp in closure'

# === Tuple unpacking in comprehensions ===
pairs = [(1, 'a'), (2, 'b'), (3, 'c')]
assert [x for x, y in pairs] == [1, 2, 3], 'unpack first element'
assert [y for x, y in pairs] == ['a', 'b', 'c'], 'unpack second element'
assert [str(x) + str(y) for x, y in [(1, 2), (3, 4)]] == ['12', '34'], 'unpack and use both'
assert [(y, x) for x, y in pairs] == [('a', 1), ('b', 2), ('c', 3)], 'swap unpacked elements'

# Tuple unpacking with filter
assert [x for x, y in pairs if x > 1] == [2, 3], 'unpack with filter'
assert [y for x, y in pairs if y != 'b'] == ['a', 'c'], 'unpack filter on second'

# Triple unpacking
triples = [(1, 2, 3), (4, 5, 6)]
assert [a + b + c for a, b, c in triples] == [6, 15], 'triple unpack sum'
assert [b for a, b, c in triples] == [2, 5], 'triple unpack middle'

# Dict comprehension with unpacking
d = {k: v for k, v in pairs}
assert d == {1: 'a', 2: 'b', 3: 'c'}, 'dict comp with unpack'
assert {v: k for k, v in pairs} == {'a': 1, 'b': 2, 'c': 3}, 'dict comp swap key/value'

# Set comprehension with unpacking
assert {x for x, y in pairs} == {1, 2, 3}, 'set comp unpack first'
assert {y for x, y in pairs} == {'a', 'b', 'c'}, 'set comp unpack second'

# Unpacking with dict.items()
d2 = {'x': 10, 'y': 20, 'z': 30}
assert [k for k, v in d2.items()] == ['x', 'y', 'z'], 'unpack dict items keys'
assert [v for k, v in d2.items()] == [10, 20, 30], 'unpack dict items values'
assert {v: k for k, v in d2.items()} == {10: 'x', 20: 'y', 30: 'z'}, 'dict comp invert dict'

# Nested comprehension with unpacking
matrix = [[(1, 2), (3, 4)], [(5, 6), (7, 8)]]
assert [[a + b for a, b in row] for row in matrix] == [[3, 7], [11, 15]], 'nested comp unpack'

# Scope isolation with unpacking (vars don't leak)
x = 'outer_x'
y = 'outer_y'
result = [x + y for x, y in [(1, 2)]]
assert x == 'outer_x', 'x does not leak from unpack'
assert y == 'outer_y', 'y does not leak from unpack'


# Unpacking in closure
def outer_unpack():
    items = [(1, 2), (3, 4)]

    def inner():
        return [a * b for a, b in items]

    return inner()


assert outer_unpack() == [2, 12], 'unpack in closure'


# Capture variable used in unpacking pattern
def outer_shadow_unpack():
    x = 100

    def inner():
        # x in unpacking shadows the outer x, but we can still reference outer x in expression
        # Actually, the x in the comprehension shadows outer x, so this tests scope isolation
        pairs = [(1, 2), (3, 4)]
        return [x + y for x, y in pairs]

    return inner()


assert outer_shadow_unpack() == [3, 7], 'shadow unpack in closure'

# === Generator expressions (temporary: treated as list comprehensions) ===
# TODO: When proper generators are implemented, these should return generator objects
# instead of lists. For now, generator expressions are parsed as list comprehensions.
# See iter__generator_expr.py for tests, and iter__generator_expr_type.py for
# a type check test (xfail=cpython since CPython has real generators).

# Generator in list() call - works identically in both Monty and CPython
assert list(x for x in [1, 2, 3]) == [1, 2, 3], 'generator in list()'
assert tuple(x for x in [1, 2, 3]) == (1, 2, 3), 'generator in tuple()'

# Generator with condition
assert list(x for x in range(10) if x % 2 == 0) == [0, 2, 4, 6, 8], 'generator with condition'

# Nested generators
assert list(x + y for x in range(3) for y in range(2)) == [0, 1, 1, 2, 2, 3], 'nested generator'

# Generator in sum()
assert sum(x for x in range(5)) == 10, 'generator in sum()'

# Generator with unpacking
pairs = [(1, 2), (3, 4)]
assert list(a + b for a, b in pairs) == [3, 7], 'generator with unpacking'

# list of strings join
assert ''.join(str(x) for x in range(5)) == '01234', 'list of strings join'
a = '1', '2', '3'
assert ''.join(a) == '123', 'tuple of strings join'
