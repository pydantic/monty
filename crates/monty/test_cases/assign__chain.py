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


# === Coverage: AssignTarget::Subscript in every chain position ===
# Subscript as the *first* (non-last) target of a 2-way chain.
sub_first = [0]
sub_first[0] = sub_first_name = 1001
assert sub_first[0] == 1001, 'subscript first: list slot written'
assert sub_first_name == 1001, 'subscript first: name gets value'

# Subscript as the *middle* target of a 3-way chain.
sub_mid = [0]
sub_mid_first = sub_mid[0] = sub_mid_last = 1002
assert sub_mid_first == 1002, 'subscript middle: first name'
assert sub_mid[0] == 1002, 'subscript middle: list slot written'
assert sub_mid_last == 1002, 'subscript middle: last name'

# Dict-subscript in the middle of a chain alongside list-subscript targets.
sub_dict_mid = {}
sub_list_mid = [0]
sub_chain_name = sub_list_mid[0] = sub_dict_mid['k'] = 1003
assert sub_chain_name == 1003 and sub_list_mid[0] == 1003 and sub_dict_mid['k'] == 1003, 'dict + list subscript chain'


# === Coverage: AssignTarget::Unpack in every chain position ===
# Tuple unpack as the *last* target, name as the first.
unpack_last_name = (uL_a, uL_b) = (11, 12)
assert unpack_last_name == (11, 12), 'unpack last: name gets tuple'
assert uL_a == 11 and uL_b == 12, 'unpack last: unpacked values'

# Tuple unpack as the *middle* target in a 3-way chain.
(uM_a, uM_b) = unpack_mid_name = unpack_mid_last = (21, 22)
assert (uM_a, uM_b) == (21, 22), 'unpack middle: first-pos unpack'
assert unpack_mid_name == (21, 22), 'unpack middle: middle-pos name'
assert unpack_mid_last == (21, 22), 'unpack middle: last-pos name'

# List-unpack syntax inside a chain.
[uli_a, uli_b] = unpack_list_name = [31, 32]
assert uli_a == 31 and uli_b == 32, 'list-unpack in chain'
assert unpack_list_name == [31, 32], 'list-unpack chain name'

# Nested tuple unpack inside a chain.
((n_x, n_y), n_z) = nested_unpack_name = ((41, 42), 43)
assert nested_unpack_name == ((41, 42), 43), 'nested unpack: whole tuple'
assert n_x == 41 and n_y == 42 and n_z == 43, 'nested unpack: leaves'

# Starred unpack inside a chain.
(*starred_rest, starred_tail) = starred_whole = [51, 52, 53, 54]
assert starred_whole == [51, 52, 53, 54], 'starred unpack: whole list'
assert starred_rest == [51, 52, 53], 'starred unpack: rest'
assert starred_tail == 54, 'starred unpack: tail'


# === Coverage: AssignTarget::Name in every chain position is already
# exercised above (first/middle/last in multiple earlier sections). ===


# === Coverage: full-mix three-way chain across Name / Subscript / Unpack ===
mix_box = [None]
(mix_a, mix_b) = mix_name = mix_box[0] = (61, 62)
assert mix_box[0] == (61, 62), 'mix chain subscript'
assert mix_name == (61, 62), 'mix chain name'
assert mix_a == 61 and mix_b == 62, 'mix chain unpack'


# === Coverage: chain inside a function where all targets become locals ===
def chain_locals_all_shapes():
    holder = [0]
    (fa, fb) = fname = holder[0] = (71, 72)
    return fname, holder[0], fa, fb


assert chain_locals_all_shapes() == ((71, 72), (71, 72), 71, 72), 'chain locals mixed shapes'


# === Coverage: long 5-way chain over Name and Subscript only ===
L = [0, 0]
D = {}
long_name = L[0] = L[1] = D['k'] = D['j'] = 808
assert long_name == 808, 'long scalar chain name'
assert L == [808, 808], 'long scalar chain list slots'
assert D == {'k': 808, 'j': 808}, 'long scalar chain dict entries'


# === Coverage: long 5-way chain that also includes Unpack (needs iterable RHS) ===
M = [(0, 0), (0, 0)]
M2 = {}
(ua, ub) = whole_name = M[0] = M[1] = M2['k'] = (9, 10)
assert whole_name == (9, 10), 'long mixed: whole-name gets tuple'
assert M == [(9, 10), (9, 10)], 'long mixed: list subscript slots'
assert M2 == {'k': (9, 10)}, 'long mixed: dict subscript slot'
assert ua == 9 and ub == 10, 'long mixed: unpack leaves'
