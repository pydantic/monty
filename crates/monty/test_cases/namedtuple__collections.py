import collections
from collections import namedtuple

# === Import surfaces ===
assert collections.namedtuple('ViaModule', ['x'])(1).x == 1, 'collections.namedtuple should be importable from the module'

# === Factory creation and repr ===
Point = namedtuple('Point', ['x', 'y'])
assert repr(Point) == "<class 'Point'>", 'namedtuple factory should have a class-like repr'

KeywordPoint = namedtuple(typename='KeywordPoint', field_names=['x', 'y'])
assert repr(KeywordPoint) == "<class 'KeywordPoint'>", 'typename and field_names should accept keyword binding'

Qualified = namedtuple('Qualified', ['x'], module='pkg.mod')
assert repr(Qualified) == "<class 'pkg.mod.Qualified'>", 'module kwarg should affect factory repr'

# === Instance construction ===
p = Point(1, 2)
assert repr(p) == 'Point(x=1, y=2)', 'instance repr should use the bare typename'
assert p.x == 1, 'attribute access should work for the first field'
assert p.y == 2, 'attribute access should work for the second field'
assert p[0] == 1, 'index access should work for the first field'
assert p[1] == 2, 'index access should work for the second field'
assert p[-1] == 2, 'negative index access should work'
assert len(p) == 2, 'namedtuple instances should report their tuple length'
assert list(p) == [1, 2], 'namedtuple instances should iterate like tuples'
assert p == (1, 2), 'namedtuple instances should compare equal to equivalent tuples'
assert (1, 2) == p, 'tuple equality should be symmetric with namedtuple instances'

# === Keyword and mixed argument binding ===
assert Point(x=1, y=2) == p, 'all-keyword construction should bind fields by name'
assert Point(1, y=2) == p, 'mixed positional and keyword construction should be supported'
assert Point(y=2, x=1) == p, 'keyword order should not matter'

# === Field-name string parsing ===
Triple = namedtuple('Triple', 'x, y z')
t = Triple(1, 2, 3)
assert (t.x, t.y, t.z) == (1, 2, 3), 'field-name strings should split on commas and whitespace'

# === Defaults are right-aligned ===
Defaults = namedtuple('Defaults', ['x', 'y', 'z'], defaults=[20, 30])
assert Defaults(1) == (1, 20, 30), 'defaults should apply to trailing fields when only required args are given'
assert Defaults(1, 2) == (1, 2, 30), 'defaults should leave explicitly provided values intact'
assert Defaults(1, 2, 3) == (1, 2, 3), 'explicit values should override defaults'

# === rename=True rewrites invalid and duplicate fields ===
Renamed = namedtuple('Renamed', ['1x', 'class', '_a', 'x', 'x'], rename=True)
r = Renamed(10, 20, 30, 40, 50)
assert r._0 == 10, 'rename=True should replace invalid identifiers with positional fallback names'
assert r._1 == 20, 'rename=True should replace keyword field names with positional fallback names'
assert r._2 == 30, 'rename=True should replace underscore-prefixed field names with positional fallback names'
assert r.x == 40, 'rename=True should preserve valid unique field names'
assert r._4 == 50, 'rename=True should replace duplicate field names with positional fallback names'
