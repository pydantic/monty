# === Basic chained assignment to names ===
a = b = c = 42
assert a == 42, 'chained name a'
assert b == 42, 'chained name b'
assert c == 42, 'chained name c'

# === Two-target chain ===
x = y = 'hello'
assert x == 'hello', 'two-target x'
assert y == 'hello', 'two-target y'

# === Chain with expression on RHS ===
p = q = 3 + 4 * 2
assert p == 11, 'chained with expression p'
assert q == 11, 'chained with expression q'

# === RHS evaluated exactly once ===
side_effects = []


def make_val():
    side_effects.append(1)
    return 99


m = n = make_val()
assert m == 99, 'chain m'
assert n == 99, 'chain n'
assert side_effects == [1], 'RHS evaluated once'

# === Chained subscript assignment ===
lst = [0, 0, 0]
d = {}
x2 = lst[0] = d['key'] = 'set'
assert x2 == 'set', 'chain name from subscript chain'
assert lst[0] == 'set', 'list subscript set in chain'
assert d['key'] == 'set', 'dict subscript set in chain'

# === Chained with tuple unpack ===
pair = (10, 20)
copy = dup = pair
assert copy == (10, 20), 'chain tuple copy'
assert dup == (10, 20), 'chain tuple dup'

(first, second) = both = (1, 2)
assert first == 1, 'unpack first in chain'
assert second == 2, 'unpack second in chain'
assert both == (1, 2), 'name from chained unpack'

# === Left-to-right ordering: earlier target sees same value as later ===
target_list = [10, 20, 30]
target_list[0] = target_list[1] = target_list[2] = 7
assert target_list == [7, 7, 7], 'all slots set to 7'

# === Chain with side-effecting subscript expressions ===
# Make sure container/index sub-expressions are evaluated at store time,
# after the RHS, in left-to-right order across targets.
order = []


def tag(name, value):
    order.append(name)
    return value


bucket_a = [0]
bucket_b = [0]
tag('rhs_outer', 0)  # sanity

# Target container expressions are evaluated lazily, once per target:
# RHS is `compute()` called once, then each target's container/index evaluates.
order.clear()


def compute():
    order.append('rhs')
    return 55


def get_a():
    order.append('a_container')
    return bucket_a


def get_b():
    order.append('b_container')
    return bucket_b


get_a()[0] = get_b()[0] = compute()
assert bucket_a[0] == 55, 'bucket a populated'
assert bucket_b[0] == 55, 'bucket b populated'
assert order == ['rhs', 'a_container', 'b_container'], f'store order {order}'

# === Chaining with augmented (op-assign) is NOT allowed in Python syntax,
# so we only cover plain `=` here. ===

# === Long chain ===
a1 = a2 = a3 = a4 = a5 = 'x'
assert a1 == 'x' and a2 == 'x' and a3 == 'x' and a4 == 'x' and a5 == 'x', 'long chain all equal'
