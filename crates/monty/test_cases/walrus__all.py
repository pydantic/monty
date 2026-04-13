# === Basic walrus operator ===
# Simple assignment expression
assert (x := 5) == 5, 'walrus returns value'
assert x == 5, 'walrus assigns to variable'

# Walrus in parentheses
y = (z := 10)
assert y == 10, 'walrus value can be assigned'
assert z == 10, 'walrus target is assigned'

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
    assert a == 3, 'walrus in if test'
else:
    assert False, 'should not reach else'

# With falsy value
if b := 0:
    assert False, 'should not reach truthy branch'
else:
    assert b == 0, 'walrus assigns even when falsy'

# === Walrus in while loops ===
counter = 0
result = []
while (n := counter) < 3:
    result.append(n)
    counter += 1
assert result == [0, 1, 2], 'walrus in while condition'
assert n == 3, 'walrus value persists after while'

# === Nested walrus ===
# Inner walrus assigned first, then outer
assert (outer := (inner := 7) + 1) == 8, 'nested walrus returns correct value'
assert inner == 7, 'inner walrus assigned'
assert outer == 8, 'outer walrus assigned'

# === Walrus in list literals ===
items = [(v := 1), v + 1, v + 2]
assert items == [1, 2, 3], 'walrus in list literal'
assert v == 1, 'walrus variable accessible after list'

# === Walrus in ternary expressions ===
result = (t := 5) if True else 0
assert result == 5, 'walrus in ternary truthy branch'
assert t == 5, 'walrus assigned in ternary'

result2 = 0 if False else (f := 6)
assert result2 == 6, 'walrus in ternary falsy branch'
assert f == 6, 'walrus assigned in falsy branch'

# === Walrus in dict/set literals ===
d = {(k := 'key'): (val := 42)}
assert d == {'key': 42}, 'walrus in dict literal'
assert k == 'key', 'walrus key assigned'
assert val == 42, 'walrus value assigned'

s = {(s1 := 1), (s2 := 2)}
assert s == {1, 2}, 'walrus in set literal'
assert s1 == 1, 'walrus in set element 1'
assert s2 == 2, 'walrus in set element 2'

# === Walrus in subscript expressions ===
arr = [10, 20, 30]
value = arr[(idx := 1)]
assert value == 20, 'walrus in subscript index'
assert idx == 1, 'walrus index assigned'


# === Walrus in function calls ===
def identity(x):
    return x


result = identity((arg := 99))
assert result == 99, 'walrus in function argument'
assert arg == 99, 'walrus arg assigned'

# === Walrus with comparison operators ===
assert (cmp := 10) > 5, 'walrus in comparison'
assert cmp == 10, 'walrus assigned in comparison'

# === Walrus in chained comparisons ===
# Note: Chained comparisons like `0 < (mid := 5) < 10` are not yet supported
# Testing a simpler comparison chain
mid = (chain := 5)
assert 0 < chain and chain < 10, 'walrus result used in comparison chain'
assert mid == 5, 'walrus assigned correctly'

# === Walrus in boolean expressions ===
# Short-circuit with and
result = (first := 1) and (second := 2)
assert result == 2, 'walrus in and expression'
assert first == 1, 'first walrus assigned'
assert second == 2, 'second walrus assigned (and evaluated)'

# Short-circuit with or (second not evaluated)
result = (or_first := 1) or (or_skip := 999)
assert result == 1, 'walrus in or expression (short-circuit)'
assert or_first == 1, 'or first walrus assigned'

# === Walrus with operations ===
assert (op := 3) + 2 == 5, 'walrus with addition'
assert op == 3, 'walrus assigned before operation'

# === Walrus in f-strings ===
msg = f'{(fvar := "hello")} world'
assert msg == 'hello world', 'walrus in f-string'
assert fvar == 'hello', 'walrus assigned in f-string'

# === Walrus with global ===
global_var = None


def set_global():
    global global_var
    return (global_var := 'set')


result = set_global()
assert result == 'set', 'walrus with global returns value'
assert global_var == 'set', 'global var assigned via walrus'


# === Walrus creates local in function scope ===
def func_scope():
    if local := 42:
        pass
    return local


assert func_scope() == 42, 'walrus creates local in function'

# === Walrus in list comprehension element (leaks to enclosing scope) ===
# Per PEP 572, walrus in comprehension assigns to enclosing scope
# Note: walrus in comprehension iterable is not allowed, but in element/condition it is
result = [(leak := x) for x in range(3)]
assert result == [0, 1, 2], 'walrus in comprehension element'
assert leak == 2, 'walrus in comprehension leaks to enclosing scope'

# === Walrus in comprehension condition ===
result = [x for x in range(5) if (limit := 3) and x < limit]
assert result == [0, 1, 2], 'walrus in comprehension condition'
assert limit == 3, 'walrus from comprehension condition accessible'

# === Multiple walrus in same expression ===
result = (m1 := 1) + (m2 := 2) + (m3 := 3)
assert result == 6, 'multiple walrus in expression'
assert m1 == 1, 'first multi-walrus'
assert m2 == 2, 'second multi-walrus'
assert m3 == 3, 'third multi-walrus'

# === Walrus in tuple ===
tup = ((t1 := 'a'), (t2 := 'b'))
assert tup == ('a', 'b'), 'walrus in tuple'
assert t1 == 'a', 'first tuple walrus'
assert t2 == 'b', 'second tuple walrus'
