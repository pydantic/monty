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
