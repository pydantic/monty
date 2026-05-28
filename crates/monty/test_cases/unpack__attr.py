# call-external
# Attribute targets in unpacking, for-loop, and comprehension contexts (issue #408).
#
# CPython allows the assignment LHS / for-loop target / comprehension `for`
# target to be an attribute access on an existing object — these mutate the
# attribute, they do not bind a new name. Requires a mutable object from the
# host; mutable dataclasses are the only sandbox-available shape with settable
# attributes today, so all sections use `make_mutable_point()`.

# === Two-target attribute unpack ===
p = make_mutable_point()
p.x, p.y = 11, 22
assert p.x == 11, 'attr unpack: first'
assert p.y == 22, 'attr unpack: second'

# === Attribute swap (RHS evaluated before stores) ===
p = make_mutable_point()
p.x = 1
p.y = 2
p.x, p.y = p.y, p.x
assert p.x == 2, 'attr swap: first'
assert p.y == 1, 'attr swap: second'

# === Attribute target with name siblings ===
p = make_mutable_point()
p.x, b = (100, 200)
assert p.x == 100, 'attr + name: attr set'
assert b == 200, 'attr + name: name bound'

# === Attribute target nested inside a tuple target ===
p = make_mutable_point()
(p.x, (p.y, c)) = (1, (2, 3))
assert p.x == 1, 'nested attr: first'
assert p.y == 2, 'nested attr: second'
assert c == 3, 'nested attr: name bound'

# === Starred middle with attribute targets at the edges ===
p = make_mutable_point()
p.x, *mid, p.y = [10, 20, 30, 40]
assert p.x == 10, 'starred middle: attr first'
assert p.y == 40, 'starred middle: attr last'
assert mid == [20, 30], 'starred middle: rest captures middle'

# === Attribute target as for-loop variable ===
p = make_mutable_point()
for p.x in (1, 2, 3):
    pass
assert p.x == 3, 'for-loop attr target retains last value'

# Attribute target as for-loop variable, visible inside body
p = make_mutable_point()
trace = []
for p.x in ('a', 'b', 'c'):
    trace.append(p.x)
assert trace == ['a', 'b', 'c'], 'for-loop attr target visible inside body'

# === Attribute target as comprehension `for` variable ===
p = make_mutable_point()
result = [p.x for p.x in (1, 2, 3)]
assert result == [1, 2, 3], 'comp attr target reads-through'
assert p.x == 3, 'comp attr target final write persists'

# Attribute target nested inside tuple in a comprehension
p = make_mutable_point()
out = [(p.x, y) for p.x, y in [(1, 'a'), (2, 'b')]]
assert out == [(1, 'a'), (2, 'b')], 'comp nested attr target reads-through'
assert p.x == 2, 'comp nested attr target final write persists'
