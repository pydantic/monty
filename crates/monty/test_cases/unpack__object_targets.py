# Attribute and subscript targets in every unpacking position.


class Point:
    def __init__(self):
        self.x = 0
        self.y = 0


# === Attribute targets ===
p = Point()
p.x, p.y = 1, 2
assert (p.x, p.y) == (1, 2)

p.x, p.y = p.y, p.x
assert (p.x, p.y) == (2, 1)

# === Subscript targets ===
d = {}
items = [0, 0, 0]
d['a'], items[1] = 'A', 'B'
assert d == {'a': 'A'}
assert items == [0, 'B', 0]

# key expressions are evaluated at store time
key = 'a'
d[key], key = 'first', 'b'
assert d == {'a': 'first'}
assert key == 'b'

# === Mixed with names and nesting ===
q = Point()
first, (q.x, d['b']) = 10, (20, 30)
assert first == 10
assert q.x == 20
assert d['b'] == 30

# === List-literal target form ===
[p.x, p.y] = [7, 8]
assert (p.x, p.y) == (7, 8)

# === Starred targets ===
head, *q.rest = [1, 2, 3, 4]
assert head == 1
assert q.rest == [2, 3, 4]

*d['init'], last = [5, 6, 7]
assert d['init'] == [5, 6]
assert last == 7

*items[0], items[1] = 'xyz'
assert items[0] == ['x', 'y']
assert items[1] == 'z'

# === Chained assignment ===
a = b = Point()
a.x, a.y = c, e = 1, 2
assert (b.x, b.y) == (1, 2)
assert (c, e) == (1, 2)

# === for loops ===
total = 0
for p.x in [1, 2, 3]:
    total += p.x
assert total == 6
assert p.x == 3

for items[0], p.y in [(1, 2), (3, 4)]:
    pass
assert items[0] == 3
assert p.y == 4


# === with ... as ===
class CM:
    def __init__(self, value):
        self.value = value

    def __enter__(self):
        return self.value

    def __exit__(self, exc_type, exc, tb):
        return False


with CM((1, 2)) as (p.x, p.y):
    pass
assert (p.x, p.y) == (1, 2)

with CM(3) as d['k']:
    pass
assert d['k'] == 3


# === Targets are read from the enclosing scope ===
def rebind():
    obj = Point()

    def inner():
        obj.x, obj.y = 5, 6

    inner()
    return obj.x, obj.y


assert rebind() == (5, 6)

# === A failing store leaves earlier targets assigned ===
p.x = 'before'
try:
    p.x, undefined_name[0] = 1, 2
    assert False, 'expected NameError'
except NameError as exc:
    assert str(exc) == "name 'undefined_name' is not defined"
assert p.x == 1

# === The usual unpacking errors still apply ===
try:
    p.x, p.y = (1, 2, 3)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'too many values to unpack (expected 2, got 3)'


# === Closures inside a target expression capture enclosing locals ===
def capture_in_targets():
    obj = Point()
    d = {}
    ((lambda: obj)()).x, ((lambda: d)())['k'] = 7, 8
    for (lambda: obj)().y in [9]:
        pass
    with CM(10) as (lambda: d)()['w']:
        pass
    return obj.x, obj.y, d


assert capture_in_targets() == (7, 9, {'k': 8, 'w': 10})
