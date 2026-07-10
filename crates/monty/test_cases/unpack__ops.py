# === Basic tuple unpacking ===
a, b = (1, 2)
assert a == 1
assert b == 2

# === Unpacking without parentheses ===
x, y = 10, 20
assert x == 10
assert y == 20

# === Three element unpacking ===
a, b, c = (1, 2, 3)
assert a == 1
assert b == 2
assert c == 3


# === Unpacking from function return ===
def returns_pair():
    return 42, 37


x, y = returns_pair()
assert x == 42
assert y == 37


def returns_triple():
    return 'a', 'b', 'c'


p, q, r = returns_triple()
assert p == 'a'
assert q == 'b'
assert r == 'c'

# === Unpacking list ===
a, b = [100, 200]
assert a == 100
assert b == 200

a, b, c, d = [1, 2, 3, 4]
assert a == 1
assert d == 4

# === Unpacking string ===
a, b = 'xy'
assert a == 'x'
assert b == 'y'

p, q, r = 'abc'
assert p == 'a'
assert q == 'b'
assert r == 'c'

# === Unpacking with different value types ===
a, b = (True, False)
assert a is True
assert b is False

a, b = (1.5, 2.5)
assert a == 1.5
assert b == 2.5

a, b = (None, 42)
assert a is None
assert b == 42

# === Unpacking with nested containers ===
a, b = ([1, 2], [3, 4])
assert a == [1, 2]
assert b == [3, 4]

a, b = ((1, 2), (3, 4))
assert a == (1, 2)
assert b == (3, 4)

# === Reassignment via unpacking ===
x = 1
y = 2
x, y = y, x
assert x == 2
assert y == 1

# === Single element tuple (edge case) ===
# Note: (x,) = (1,) is valid Python
(a,) = (42,)
assert a == 42

(a,) = [99]
assert a == 99

(a,) = 'z'
assert a == 'z'

# === Star unpacking (extended unpacking) ===
# Star at end
first, *rest = [1, 2, 3, 4, 5]
assert first == 1
assert rest == [2, 3, 4, 5]

# Star at start
*init, last = [1, 2, 3, 4, 5]
assert init == [1, 2, 3, 4]
assert last == 5

# Star in middle
first, *middle, last = [1, 2, 3, 4, 5]
assert first == 1
assert middle == [2, 3, 4]
assert last == 5

# Empty rest (minimum values)
first, *rest, last = [1, 2]
assert first == 1
assert rest == []
assert last == 2

# From tuple
a, *b = (10, 20, 30)
assert a == 10
assert b == [20, 30]

# From string
first, *mid, last = 'abcde'
assert first == 'a'
assert mid == ['b', 'c', 'd']
assert last == 'e'

# With more targets before star
a, b, c, *rest = [1, 2, 3, 4, 5, 6]
assert a == 1
assert b == 2
assert c == 3
assert rest == [4, 5, 6]

# With more targets after star
*init, x, y, z = [1, 2, 3, 4, 5, 6]
assert init == [1, 2, 3]
assert x == 4
assert y == 5
assert z == 6

# Star captures all but one
head, *tail = [1]
assert head == 1
assert tail == []

# Star with bracket syntax
[a, *b, c] = [1, 2, 3, 4]
assert a == 1
assert b == [2, 3]
assert c == 4
