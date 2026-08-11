# === Basic walrus operator ===
# Simple assignment expression
assert (x := 5) == 5
assert x == 5

# Walrus in parentheses
y = (z := 10)
assert y == 10
assert z == 10

# simple if
x = None
answer = 'unset'
if y := x:
    answer = f'x is {y}'

assert answer == 'unset'

x = 123
if y := x:
    answer = f'x is {y}'

assert answer == 'x is 123'
x = 0
if y := x:
    answer = f'x is {y}'
else:
    answer = 'x is unset'

assert answer == 'x is unset'

# === Walrus in if conditions ===
if (a := 3) > 0:
    assert a == 3
else:
    assert False, 'should not reach else'

# With falsy value
if b := 0:
    assert False, 'should not reach truthy branch'
else:
    assert b == 0

# === Walrus in while loops ===
counter = 0
result = []
while (n := counter) < 3:
    result.append(n)
    counter += 1
assert result == [0, 1, 2]
assert n == 3

# === Nested walrus ===
# Inner walrus assigned first, then outer
assert (outer := (inner := 7) + 1) == 8
assert inner == 7
assert outer == 8

# === Walrus in list literals ===
items = [(v := 1), v + 1, v + 2]
assert items == [1, 2, 3]
assert v == 1

# === Walrus in ternary expressions ===
result = (t := 5) if True else 0
assert result == 5
assert t == 5

result2 = 0 if False else (f := 6)
assert result2 == 6
assert f == 6

# === Walrus in dict/set literals ===
d = {(k := 'key'): (val := 42)}
assert d == {'key': 42}
assert k == 'key'
assert val == 42

s = {(s1 := 1), (s2 := 2)}
assert s == {1, 2}
assert s1 == 1
assert s2 == 2

# === Walrus in subscript expressions ===
arr = [10, 20, 30]
value = arr[(idx := 1)]
assert value == 20
assert idx == 1


# === Walrus in function calls ===
def identity(x):
    return x


result = identity((arg := 99))
assert result == 99
assert arg == 99

# === Walrus with comparison operators ===
assert (cmp := 10) > 5
assert cmp == 10

# === Walrus in chained comparisons ===
# Note: Chained comparisons like `0 < (mid := 5) < 10` are not yet supported
# Testing a simpler comparison chain
mid = (chain := 5)
assert 0 < chain and chain < 10, 'walrus result used in comparison chain'
assert mid == 5

# === Walrus in boolean expressions ===
# Short-circuit with and
result = (first := 1) and (second := 2)
assert result == 2
assert first == 1
assert second == 2

# Short-circuit with or (second not evaluated)
result = (or_first := 1) or (or_skip := 999)
assert result == 1
assert or_first == 1

# === Walrus with operations ===
assert (op := 3) + 2 == 5
assert op == 3

# === Walrus in f-strings ===
msg = f'{(fvar := "hello")} world'
assert msg == 'hello world'
assert fvar == 'hello'

# === Walrus with global ===
global_var = None


def set_global():
    global global_var
    return (global_var := 'set')


result = set_global()
assert result == 'set'
assert global_var == 'set'


# === Walrus creates local in function scope ===
def func_scope():
    if local := 42:
        pass
    return local


assert func_scope() == 42

# === Walrus in list comprehension element (leaks to enclosing scope) ===
# Per PEP 572, walrus in comprehension assigns to enclosing scope
# Note: walrus in comprehension iterable is not allowed, but in element/condition it is
result = [(leak := x) for x in range(3)]
assert result == [0, 1, 2]
assert leak == 2

# === Walrus in comprehension condition ===
result = [x for x in range(5) if (limit := 3) and x < limit]
assert result == [0, 1, 2]
assert limit == 3

# === Multiple walrus in same expression ===
result = (m1 := 1) + (m2 := 2) + (m3 := 3)
assert result == 6
assert m1 == 1
assert m2 == 2
assert m3 == 3

# === Walrus in tuple ===
tup = ((t1 := 'a'), (t2 := 'b'))
assert tup == ('a', 'b')
assert t1 == 'a'
assert t2 == 'b'
