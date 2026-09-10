# call-external
# === Basic dataclass tests ===

# Get dataclass instance from external function
point = make_point()

# === repr and str ===
assert repr(point) == 'Point(x=1, y=2)', f'point repr {point=!r}'
assert str(point) == 'Point(x=1, y=2)'

# === Boolean truthiness ===
# Dataclasses are always truthy (like Python class instances)
assert bool(point)

# === Host instances are unhashable ===
# A dataclass with eq (the default) and no frozen=True defines __eq__ without
# __hash__, so instances are unhashable — in CPython and in the sandbox.
point2 = make_point()
try:
    hash(point)
    assert False, 'expected TypeError from hash()'
except TypeError as e:
    assert str(e) == "unhashable type: 'Point'"

alice = make_user('Alice')
bob = make_user('Bob')

# === Equality ===
assert point == point2
assert alice != bob
assert point != alice
# A dataclass is never equal to a non-dataclass, even with matching fields
assert point != (1, 2)
assert point != {'x': 1, 'y': 2}

# === Mutable dataclass ===
mut_point = make_mutable_point()
assert repr(mut_point) == 'MutablePoint(x=1, y=2)', f'mutable point repr {mut_point=!r}'
# Distinct classes are not equal even with identical field names and values
# (Point and MutablePoint are both (x=1, y=2)).
assert point != mut_point
assert mut_point != point

# === Dataclass with string argument ===
alice = make_user('Alice')
assert repr(alice) == "User(name='Alice', active=True)", f'user repr with string field {alice=!r}'

# === Dataclass in list (using existing variables) ===
points = [point, mut_point, alice]
assert len(points) == 3

# === Attribute access (get) ===
# Access fields on immutable dataclass
assert point.x == 1
assert point.y == 2

# Access fields on mutable dataclass
assert mut_point.x == 1
assert mut_point.y == 2

# Access fields on dataclass with string field
assert alice.name == 'Alice'
assert alice.active == True

# === Attribute assignment (set) ===
# Modify mutable dataclass
mut_point.x = 10
assert mut_point.x == 10
mut_point.y = 20
assert mut_point.y == 20
assert repr(mut_point) == 'MutablePoint(x=10, y=20)', f'repr after attribute update {mut_point=!r}'

# === set other attributes
# NOTE: repr is deliberately not asserted here — Monty shows all eager attrs
# (including z), CPython's dataclass repr shows declared fields only
# (see limitations/classes.md).
mut_point.z = 30
assert mut_point.z == 30

# === Augmented attribute assignment (+=, -=, etc.) ===
aug_point = make_mutable_point()
aug_point.x += 5
assert aug_point.x == 6
aug_point.y -= 1
assert aug_point.y == 1
aug_point.x *= 3
assert aug_point.x == 18

# === Chained augmented attribute assignment ===
outer_aug = make_mutable_point()
inner_aug = make_mutable_point()
outer_aug.x = inner_aug
outer_aug.x.y += 100
assert inner_aug.y == 102

# === Chained attribute assignment (a.x = b.x = value) ===
ca = make_mutable_point()
cb = make_mutable_point()
ca.x = cb.x = 77
assert ca.x == 77
assert cb.x == 77

# === Chained mixed attribute/name/subscript assignment ===
# Attribute in the *middle* of a chain, between a name and a subscript target.
holder = [0]
cm = make_mutable_point()
val = cm.y = holder[0] = 321
assert val == 321
assert cm.y == 321
assert holder[0] == 321

# === Attribute as the *last* target of a chain ===
attr_last_name_box = [0]
attr_last_obj = make_mutable_point()
attr_last_name = attr_last_name_box[0] = attr_last_obj.y = 555
assert attr_last_name == 555
assert attr_last_name_box[0] == 555
assert attr_last_obj.y == 555

# === Attribute as the *first* (non-last) target of a chain ===
attr_first_obj = make_mutable_point()
attr_first_obj.x = attr_first_name = 556
assert attr_first_obj.x == 556
assert attr_first_name == 556

# === Three attribute targets in one chain ===
pa = make_mutable_point()
pb = make_mutable_point()
pc = make_mutable_point()
pa.x = pb.x = pc.x = 999
assert pa.x == 999
assert pb.x == 999
assert pc.x == 999

# === Nested attribute target as part of a chain ===
nest_outer = make_mutable_point()
nest_inner = make_mutable_point()
nest_outer.x = nest_inner
nested_chain_name = nest_outer.x.y = 444
assert nested_chain_name == 444
assert nest_inner.y == 444
assert nest_outer.x.y == 444

# === Chain with attribute + tuple unpack into attribute values ===
# Each chain step must see the same RHS value.
unpack_dst = make_mutable_point()
(ua1, ua2) = unpack_dst.x = unpack_chain_name = (13, 14)
assert ua1 == 13 and ua2 == 14, 'attr-unpack chain: unpack'
assert unpack_dst.x == (13, 14)
assert unpack_chain_name == (13, 14)

# === Nested attribute access (chained get) ===
# Create outer dataclass with inner dataclass as field
outer = make_mutable_point()
inner = make_mutable_point()
inner.x = 100
inner.y = 200
outer.x = inner

# Chained attribute get: outer.x.y
assert outer.x.x == 100
assert outer.x.y == 200

# === Nested attribute assignment (chained set) ===
# Modify nested field via chained access
outer.x.x = 999
assert outer.x.x == 999
outer.x.y = 888
assert outer.x.y == 888

# Verify inner was modified (same object)
assert inner.x == 999
assert inner.y == 888

# === Deeper nesting (3 levels) ===
level1 = make_mutable_point()
level2 = make_mutable_point()
level3 = make_mutable_point()
level3.x = 42
level2.x = level3
level1.x = level2

# 3-level chained get
assert level1.x.x.x == 42

# 3-level chained set
level1.x.x.x = 7
assert level1.x.x.x == 7
assert level3.x == 7

# === Empty dataclass ===
empty = make_empty()
assert repr(empty) == 'Empty()'
assert str(empty) == 'Empty()'

# === setattr mutates the sandbox copy only ===
copy_point = make_point()
copy_point.x = 10
assert copy_point.x == 10
# a fresh instance from the host is unaffected
assert make_point().x == 1

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

# === type() of host class instances names the real class ===
# NOTE: repr(type(x)) is deliberately not asserted — CPython qualifies it with
# the defining module ("<class 'test_fixtures.Point'>"), Monty shows the bare
# class name (see limitations/classes.md).
type_pt = make_point()
type_pt2 = make_point()
type_mut = make_mutable_point()
assert type(type_pt).__name__ == 'Point'
assert type(type_pt) == type(type_pt2)
assert type(type_pt) != type(type_mut)
# equal type objects collide in sets (hash consistent with eq)
assert len({type(type_pt), type(type_pt2)}) == 1
assert len({type(type_pt), type(type_mut)}) == 2

# === Error messages name the real class, not a placeholder ===
try:
    hash(type_mut)
    assert False, 'should have raised TypeError for unhashable mutable instance'
except TypeError as e:
    assert str(e) == "unhashable type: 'MutablePoint'", f'wrong message: {e}'

# === Lazy attribute lookups (class attributes served by the host) ===
# `dimensions` is a class attribute, not a field: CPython resolves it via
# class lookup, Monty suspends a NameLookup routed by instance_id.
assert point.dimensions == 2
assert mut_point.dimensions == 2
# repeated access re-consults the host (no caching) and stays consistent
assert point.dimensions == 2
# lazy attrs are not part of repr or equality
assert repr(point) == 'Point(x=1, y=2)'
# a locally mutated instance still allows lazy attribute reads
assert copy_point.dimensions == 2

# === Lazy lookup answered Undefined raises AttributeError ===
try:
    alice2.dimensions
    assert False, 'should have raised AttributeError for lazy attr on User'
except AttributeError as e:
    assert str(e) == "'User' object has no attribute 'dimensions'", f'wrong message: {e}'

# === getattr()/hasattr() consult the host like obj.attr ===
assert hasattr(point, 'dimensions') is True
assert getattr(point, 'dimensions') == 2
assert getattr(point, 'dimensions', 99) == 2
assert hasattr(alice2, 'dimensions') is False
assert getattr(alice2, 'dimensions', 99) == 99
try:
    getattr(alice2, 'dimensions')
    assert False, 'should have raised AttributeError for getattr of a lazy attr on User'
except AttributeError as e:
    assert str(e) == "'User' object has no attribute 'dimensions'", f'wrong message: {e}'

# === Default protocol errors name the real class, not 'HostClass' ===
proto_point = make_mutable_point()
try:
    proto_point[0]
    assert False, 'expected subscript to fail'
except TypeError as e:
    assert str(e) == "'MutablePoint' object is not subscriptable"
try:
    proto_point[0] = 1
    assert False, 'expected item assignment to fail'
except TypeError as e:
    assert str(e) == "'MutablePoint' object does not support item assignment"
try:
    for _ in proto_point:
        pass
    assert False, 'expected iteration to fail'
except TypeError as e:
    assert str(e) == "'MutablePoint' object is not iterable"
try:
    next(proto_point)
    assert False, 'expected next() to fail'
except TypeError as e:
    assert str(e) == "'MutablePoint' object is not an iterator"
try:
    proto_point()
    assert False, 'expected call to fail'
except TypeError as e:
    assert str(e) == "'MutablePoint' object is not callable"

# === A callable set on the sandbox copy is called as-is ===
proto_point.f = lambda: 42
assert proto_point.f() == 42
proto_point.g = lambda a, b=1: a + b
assert proto_point.g(2) == 3
assert proto_point.g(2, b=5) == 7
try:
    proto_point.x()
    assert False, 'expected calling a data attribute to fail'
except TypeError as e:
    assert str(e) == "'int' object is not callable"

# === Lazy attribute errors propagate ===
# `MutablePoint.boom` is a property that raises KeyError on the host. Only
# AttributeError means "absent": anything else is raised in the sandbox as-is,
# so hasattr() does not swallow it and getattr() does not fall back to its
# default — exactly as CPython treats a raising property.
try:
    mut_point.boom
    assert False, 'expected KeyError from the boom property'
except KeyError as e:
    assert str(e) == "'boom'"
try:
    hasattr(mut_point, 'boom')
    assert False, 'expected hasattr() to propagate KeyError'
except KeyError as e:
    assert str(e) == "'boom'"
try:
    getattr(mut_point, 'boom', 1)
    assert False, 'expected getattr() with a default to propagate KeyError'
except KeyError as e:
    assert str(e) == "'boom'"
