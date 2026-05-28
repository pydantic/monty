# === Basic tuple unpacking ===
a, b = (1, 2)
assert a == 1, 'first element of tuple'
assert b == 2, 'second element of tuple'

# === Unpacking without parentheses ===
x, y = 10, 20
assert x == 10, 'first element without parens'
assert y == 20, 'second element without parens'

# === Three element unpacking ===
a, b, c = (1, 2, 3)
assert a == 1, 'three elements: first'
assert b == 2, 'three elements: second'
assert c == 3, 'three elements: third'


# === Unpacking from function return ===
def returns_pair():
    return 42, 37


x, y = returns_pair()
assert x == 42, 'function return first'
assert y == 37, 'function return second'


def returns_triple():
    return 'a', 'b', 'c'


p, q, r = returns_triple()
assert p == 'a', 'function return triple first'
assert q == 'b', 'function return triple second'
assert r == 'c', 'function return triple third'

# === Unpacking list ===
a, b = [100, 200]
assert a == 100, 'list unpack first'
assert b == 200, 'list unpack second'

a, b, c, d = [1, 2, 3, 4]
assert a == 1, 'four element list first'
assert d == 4, 'four element list fourth'

# === Unpacking string ===
a, b = 'xy'
assert a == 'x', 'string unpack first char'
assert b == 'y', 'string unpack second char'

p, q, r = 'abc'
assert p == 'a', 'three char string first'
assert q == 'b', 'three char string second'
assert r == 'c', 'three char string third'

# === Unpacking with different value types ===
a, b = (True, False)
assert a is True, 'bool tuple first'
assert b is False, 'bool tuple second'

a, b = (1.5, 2.5)
assert a == 1.5, 'float tuple first'
assert b == 2.5, 'float tuple second'

a, b = (None, 42)
assert a is None, 'mixed tuple None'
assert b == 42, 'mixed tuple int'

# === Unpacking with nested containers ===
a, b = ([1, 2], [3, 4])
assert a == [1, 2], 'nested list first'
assert b == [3, 4], 'nested list second'

a, b = ((1, 2), (3, 4))
assert a == (1, 2), 'nested tuple first'
assert b == (3, 4), 'nested tuple second'

# === Reassignment via unpacking ===
x = 1
y = 2
x, y = y, x
assert x == 2, 'swap first'
assert y == 1, 'swap second'

# === Single element tuple (edge case) ===
# Note: (x,) = (1,) is valid Python
(a,) = (42,)
assert a == 42, 'single element tuple unpack'

(a,) = [99]
assert a == 99, 'single element list unpack'

(a,) = 'z'
assert a == 'z', 'single char string unpack'

# === Star unpacking (extended unpacking) ===
# Star at end
first, *rest = [1, 2, 3, 4, 5]
assert first == 1, 'star at end: first'
assert rest == [2, 3, 4, 5], 'star at end: rest'

# Star at start
*init, last = [1, 2, 3, 4, 5]
assert init == [1, 2, 3, 4], 'star at start: init'
assert last == 5, 'star at start: last'

# Star in middle
first, *middle, last = [1, 2, 3, 4, 5]
assert first == 1, 'star in middle: first'
assert middle == [2, 3, 4], 'star in middle: middle'
assert last == 5, 'star in middle: last'

# Empty rest (minimum values)
first, *rest, last = [1, 2]
assert first == 1, 'empty rest: first'
assert rest == [], 'empty rest: rest is empty list'
assert last == 2, 'empty rest: last'

# From tuple
a, *b = (10, 20, 30)
assert a == 10, 'star from tuple: a'
assert b == [20, 30], 'star from tuple: b is list'

# From string
first, *mid, last = 'abcde'
assert first == 'a', 'star from string: first'
assert mid == ['b', 'c', 'd'], 'star from string: mid'
assert last == 'e', 'star from string: last'

# With more targets before star
a, b, c, *rest = [1, 2, 3, 4, 5, 6]
assert a == 1, 'multiple before star: a'
assert b == 2, 'multiple before star: b'
assert c == 3, 'multiple before star: c'
assert rest == [4, 5, 6], 'multiple before star: rest'

# With more targets after star
*init, x, y, z = [1, 2, 3, 4, 5, 6]
assert init == [1, 2, 3], 'multiple after star: init'
assert x == 4, 'multiple after star: x'
assert y == 5, 'multiple after star: y'
assert z == 6, 'multiple after star: z'

# Star captures all but one
head, *tail = [1]
assert head == 1, 'single item: head'
assert tail == [], 'single item: tail is empty'

# Star with bracket syntax
[a, *b, c] = [1, 2, 3, 4]
assert a == 1, 'bracket syntax: a'
assert b == [2, 3], 'bracket syntax: b'
assert c == 4, 'bracket syntax: c'

# === Subscript targets in unpack assignment (issue #408) ===
# Single subscript target in a 1-tuple LHS
x = [0]
(x[0],) = (5,)
assert x[0] == 5, 'single subscript target unpack'

# Two subscript targets
a = [0, 0]
a[0], a[1] = 1, 2
assert a == [1, 2], 'two subscript targets'

# Swap two elements via subscript targets (CPython evaluates RHS before stores)
a = [1, 2]
a[0], a[1] = a[1], a[0]
assert a == [2, 1], 'subscript swap'

# Subscript target nested inside a tuple target
a = [0, 0]
(a[0], (a[1], b)) = (1, (2, 3))
assert a == [1, 2], 'nested: subscripts updated'
assert b == 3, 'nested: name bound'

# Subscript target with name siblings
a = [0]
a[0], b = (10, 20)
assert a == [10], 'subscript + name: subscript updated'
assert b == 20, 'subscript + name: name bound'

# Starred middle with subscript targets at the edges
a = [0, 0]
a[0], *rest, a[1] = [10, 20, 30, 40]
assert a == [10, 40], 'starred middle: subscript edges'
assert rest == [20, 30], 'starred middle: rest captures middle'

# Walrus inside subscript index — bind survives the unpack
a = [0, 0]
(a[(i := 1)],) = (5,)
assert a == [0, 5], 'walrus in subscript index'
assert i == 1, 'walrus binding survives unpack'

# Side-effecting index expression evaluates at store time, not before
calls = []


def idx():
    calls.append('idx')
    return 0


a = [0]
(a[idx()],) = (7,)
assert a == [7], 'side-effecting index produces correct store'
assert calls == ['idx'], 'index expression evaluated exactly once at store time'


# Lambda inside a `for`-loop subscript-target index — exercises the scope
# walker that must promote captured outer locals to cell-vars when the only
# reference to them lives inside the loop target's sub-expressions.
def _for_target_lambda():
    x = 1
    y = [0, 0]
    for y[(lambda: x)()] in (10, 20):
        pass
    return y


assert _for_target_lambda() == [0, 20], 'lambda capturing local from inside for-target subscript index'


# Same pattern wrapped in a nested tuple target — exercises the Tuple
# recursion arm in the cell-var / referenced-name unpack-target walkers.
def _for_target_tuple_lambda():
    x = 1
    y = [0, 0]
    z = ''
    for y[(lambda: x)()], z in [(10, 'a'), (20, 'b')]:
        pass
    return y, z


assert _for_target_tuple_lambda() == ([0, 20], 'b'), 'tuple target with lambda-in-subscript-index'
