# call-external
# === Basic dataclass tests ===

# Get immutable dataclass from external function
point = make_point()

# === repr and str ===
assert repr(point) == 'Point(x=1, y=2)', f'point repr {point=!r}'
assert str(point) == 'Point(x=1, y=2)', 'point str'

# === Boolean truthiness ===
# Dataclasses are always truthy (like Python class instances)
assert bool(point), 'dataclass bool is True'

# === Hash for immutable dataclass ===
# Immutable (frozen) dataclasses are hashable
h1 = hash(point)
assert h1 != 0, 'hash is not zero'

# Hash is consistent - same object hashes to same value
h2 = hash(point)
assert h1 == h2, 'hash is consistent'

# Equal frozen dataclasses hash to same value
point2 = make_point()
assert hash(point) == hash(point2), 'equal dataclasses have equal hash'

# Frozen dataclass can be used as dict key
d = {point: 'first'}
assert d[point] == 'first', 'frozen dataclass as dict key'
assert d[point2] == 'first', 'equal frozen dataclass looks up same value'

# Frozen dataclass can be added to set
s = {point, point2}
assert len(s) == 1, 'equal frozen dataclasses deduplicated in set'

# Different field values produce different hash
alice = make_user('Alice')
bob = make_user('Bob')
assert hash(alice) != hash(bob), 'different field values have different hash'

# === Mutable dataclass ===
mut_point = make_mutable_point()
assert repr(mut_point) == 'MutablePoint(x=1, y=2)', f'mutable point repr {mut_point=!r}'

# === Dataclass with string argument ===
alice = make_user('Alice')
assert repr(alice) == "User(name='Alice', active=True)", f'user repr with string field {alice=!r}'

# === Dataclass in list (using existing variables) ===
points = [point, mut_point, alice]
assert len(points) == 3, 'dataclass list length'

# === Attribute access (get) ===
# Access fields on immutable dataclass
assert point.x == 1, 'point.x is 1'
assert point.y == 2, 'point.y is 2'

# Access fields on mutable dataclass
assert mut_point.x == 1, 'mut_point.x is 1'
assert mut_point.y == 2, 'mut_point.y is 2'

# Access fields on dataclass with string field
assert alice.name == 'Alice', 'alice.name is Alice'
assert alice.active == True, 'alice.active is True'

# === Attribute assignment (set) ===
# Modify mutable dataclass
mut_point.x = 10
assert mut_point.x == 10, 'mut_point.x updated to 10'
mut_point.y = 20
assert mut_point.y == 20, 'mut_point.y updated to 20'
assert repr(mut_point) == 'MutablePoint(x=10, y=20)', f'repr after attribute update {mut_point=!r}'

# === set other attributes
mut_point.z = 30
assert mut_point.z == 30, 'mut_point.z updated to 30'
assert repr(mut_point) == 'MutablePoint(x=10, y=20)', 'repr after attribute update'

# === Nested attribute access (chained get) ===
# Create outer dataclass with inner dataclass as field
outer = make_mutable_point()
inner = make_mutable_point()
inner.x = 100
inner.y = 200
outer.x = inner

# Chained attribute get: outer.x.y
assert outer.x.x == 100, 'outer.x.x is 100'
assert outer.x.y == 200, 'outer.x.y is 200'

# === Nested attribute assignment (chained set) ===
# Modify nested field via chained access
outer.x.x = 999
assert outer.x.x == 999, 'outer.x.x updated to 999'
outer.x.y = 888
assert outer.x.y == 888, 'outer.x.y updated to 888'

# Verify inner was modified (same object)
assert inner.x == 999, 'inner.x also updated to 999'
assert inner.y == 888, 'inner.y also updated to 888'

# === Deeper nesting (3 levels) ===
level1 = make_mutable_point()
level2 = make_mutable_point()
level3 = make_mutable_point()
level3.x = 42
level2.x = level3
level1.x = level2

# 3-level chained get
assert level1.x.x.x == 42, 'level1.x.x.x is 42'

# 3-level chained set
level1.x.x.x = 7
assert level1.x.x.x == 7, 'level1.x.x.x updated to 7'
assert level3.x == 7, 'level3.x also updated to 7'

# === Empty dataclass ===
empty = make_empty()
assert repr(empty) == 'Empty()', 'empty dataclass repr'
assert str(empty) == 'Empty()', 'empty dataclass str'

# === FrozenInstanceError is subclass of AttributeError ===
# Catching AttributeError should also catch FrozenInstanceError
frozen_point = make_point()
caught = False
try:
    frozen_point.x = 10
except AttributeError:
    caught = True
assert caught, 'FrozenInstanceError caught by AttributeError'

# === Error: accessing non-existent attribute ===
try:
    point.nonexistent
    assert False, 'should have raised AttributeError for missing attr'
except AttributeError as e:
    assert str(e) == "'Point' object has no attribute 'nonexistent'", f'wrong message: {e}'

# === Error: accessing non-existent private attribute ===
try:
    point._private
    assert False, 'should have raised AttributeError for private attr'
except AttributeError as e:
    assert str(e) == "'Point' object has no attribute '_private'", f'wrong message: {e}'

# === Error: calling a dunder that doesn't exist ===
try:
    point.__nonexistent__()
    assert False, 'should have raised AttributeError for dunder'
except AttributeError as e:
    assert str(e) == "'Point' object has no attribute '__nonexistent__'", f'wrong message: {e}'

# === Error: calling a private method that doesn't exist ===
try:
    point._private_method()
    assert False, 'should have raised AttributeError for private method'
except AttributeError as e:
    assert str(e) == "'Point' object has no attribute '_private_method'", f'wrong message: {e}'

# === Error: calling a field value (not callable) ===
try:
    point.x()
    assert False, 'should have raised TypeError for calling int field'
except TypeError as e:
    assert str(e) == "'int' object is not callable", f'wrong message: {e}'

# === Error: calling a non-existent public method ===
try:
    point.nonexistent_method()
    assert False, 'should have raised AttributeError for missing method'
except AttributeError as e:
    assert str(e) == "'Point' object has no attribute 'nonexistent_method'", f'wrong message: {e}'

# === Error: same errors on mutable dataclass ===
try:
    mut_point.nonexistent
    assert False, 'should have raised AttributeError on mutable dc'
except AttributeError as e:
    assert str(e) == "'MutablePoint' object has no attribute 'nonexistent'", f'wrong message: {e}'

try:
    mut_point.x()
    assert False, 'should have raised TypeError on mutable dc field call'
except TypeError as e:
    assert str(e) == "'int' object is not callable", f'wrong message: {e}'

# === Method calls: no args (exercises ArgValues::prepend on Empty) ===
result = point.sum()
assert result == 3, f'Point.sum() should be 3, got {result}'

# === Method calls: two positional args (exercises ArgValues::prepend on Two) ===
new_point = point.add(10, 20)
assert new_point.x == 11, f'Point.add x should be 11, got {new_point.x}'
assert new_point.y == 22, f'Point.add y should be 22, got {new_point.y}'

# === Method calls: one positional arg (exercises ArgValues::prepend on One) ===
scaled = point.scale(3)
assert scaled.x == 3, f'Point.scale x should be 3, got {scaled.x}'
assert scaled.y == 6, f'Point.scale y should be 6, got {scaled.y}'

# === Method calls: returning a string ===
desc = point.describe('pt')
assert desc == 'pt(1, 2)', f'Point.describe should be pt(1, 2), got {desc}'

# === Method calls on mutable dataclass ===
mut_p2 = make_mutable_point()
mut_sum = mut_p2.sum()
assert mut_sum == 3, f'MutablePoint.sum() should be 3, got {mut_sum}'

# === Method calls on User dataclass (string field) ===
alice2 = make_user('Alice')
greeting = alice2.greeting()
assert greeting == 'Hello, Alice!', f'User.greeting should be Hello, Alice!, got {greeting}'

# === Method call returning dataclass - chained access ===
p3 = point.add(0, 0)
assert p3.x == 1, f'chained method access: p3.x should be 1, got {p3.x}'
assert p3.y == 2, f'chained method access: p3.y should be 2, got {p3.y}'

# === Method calls with keyword-only args (exercises ArgValues::prepend on Kwargs) ===
desc_kw = point.describe(label='custom')
assert desc_kw == 'custom(1, 2)', f'Point.describe(label=) should be custom(1, 2), got {desc_kw}'

# === Error: calling non-existent method on mutable dataclass ===
try:
    mut_p2.nonexistent_method()
    assert False, 'should have raised AttributeError for missing method on mutable dc'
except AttributeError as e:
    assert str(e) == "'MutablePoint' object has no attribute 'nonexistent_method'", f'wrong message: {e}'

# === Error: calling non-existent method on User ===
try:
    alice2.missing()
    assert False, 'should have raised AttributeError for missing method on User'
except AttributeError as e:
    assert str(e) == "'User' object has no attribute 'missing'", f'wrong message: {e}'
