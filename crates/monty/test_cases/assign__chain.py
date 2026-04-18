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
# Verify that the RHS runs once, and that each target's container *and* index
# sub-expressions are evaluated lazily at store time, in left-to-right order
# across targets and interleaved container→index within each target.
order = []
bucket_a = [0]
bucket_b = [0]


def compute():
    order.append('rhs')
    return 55


def get_a():
    order.append('a_container')
    return bucket_a


def idx_a():
    order.append('a_index')
    return 0


def get_b():
    order.append('b_container')
    return bucket_b


def idx_b():
    order.append('b_index')
    return 0


get_a()[idx_a()] = get_b()[idx_b()] = compute()
assert bucket_a[0] == 55, 'bucket a populated'
assert bucket_b[0] == 55, 'bucket b populated'
assert order == ['rhs', 'a_container', 'a_index', 'b_container', 'b_index'], f'store order {order}'

# === Chaining with augmented (op-assign) is NOT allowed in Python syntax,
# so we only cover plain `=` here. ===

# === Long chain ===
a1 = a2 = a3 = a4 = a5 = 'x'
assert a1 == 'x' and a2 == 'x' and a3 == 'x' and a4 == 'x' and a5 == 'x', 'long chain all equal'


# === Chained assignment in function scope (all targets become locals) ===
def fn_locals():
    la = lb = lc = 100
    return la, lb, lc


assert fn_locals() == (100, 100, 100), 'chained locals'


# === Chained assignment through `global` ===
g1 = g2 = 0


def set_globals():
    global g1, g2
    g1 = g2 = 77


set_globals()
assert g1 == 77, 'chained global g1'
assert g2 == 77, 'chained global g2'


# === Chained assignment mixing a local and a global ===
g3 = 0


def mix_local_global():
    global g3
    loc = g3 = 88
    return loc


assert mix_local_global() == 88, 'chain local gets value'
assert g3 == 88, 'chain global gets value'


# === Chained assignment through `nonlocal` ===
def set_nonlocals():
    x = y = 0

    def inner():
        nonlocal x, y
        x = y = 123

    inner()
    return x, y


assert set_nonlocals() == (123, 123), 'chained nonlocal targets'


# === Chained assignment mixing a local and a nonlocal ===
def mix_local_nonlocal():
    x = 0

    def inner():
        nonlocal x
        local = x = 222
        return local

    local = inner()
    return local, x


assert mix_local_nonlocal() == (222, 222), 'chain local and nonlocal'


# === Walrus inside RHS of a chained assignment ===
# The walrus binds `cc` before any target store; both `aa` and `bb` then receive
# the post-walrus expression result.
def walrus_in_chain():
    aa = bb = (cc := 55) + 1
    return aa, bb, cc


assert walrus_in_chain() == (56, 56, 55), 'walrus binds before targets'


# === UnboundLocalError: subscript container evaluated before its own assignment ===
# `lst` is a local (it is one of the chain targets), so at store time of `lst[0]`
# the name `lst` has no value yet and evaluating the container must raise.
def unbound_subscript():
    try:
        lst[0] = lst = [1, 2, 3]
    except UnboundLocalError:
        return 'unbound'
    return 'no-error'


assert unbound_subscript() == 'unbound', 'subscript target container sees unbound local'


# === TypeError: name store happens first, later subscript target sees wrong type ===
# First store: `nm` becomes the int 1. Second store evaluates `nm[0]` on an int,
# which is not subscriptable.
def type_error_after_name():
    try:
        nm = nm[0] = 1
    except TypeError:
        return 'type-error'
    return 'no-error'


assert type_error_after_name() == 'type-error', 'later subscript target sees updated binding'
