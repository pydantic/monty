# mode: iter
# === Basic dataclass tests ===

# Get immutable dataclass from external function
point = make_point()

# === repr and str ===
assert repr(point) == 'Point(x=1, y=2)', 'point repr'
assert str(point) == 'Point(x=1, y=2)', 'point str'

# === Boolean truthiness ===
# Dataclasses are always truthy (like Python class instances)
assert bool(point), 'dataclass bool is True'

# === Hash for immutable dataclass ===
# Immutable dataclasses are hashable
h1 = hash(point)
assert h1 != 0, 'hash is not zero'

# === Mutable dataclass ===
mut_point = make_mutable_point()
assert repr(mut_point) == 'Point(x=1, y=2)', 'mutable point repr'

# === Dataclass with string argument ===
alice = make_user('Alice')
assert repr(alice) == "User(name='Alice', active=True)", 'user repr with string field'

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
assert repr(mut_point) == 'Point(x=10, y=20)', 'repr after attribute update'
