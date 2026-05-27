# === Basic global read/write ===
x1 = 42


def read_explicit():
    global x1
    return x1


assert read_explicit() == 42, 'explicit global read'


x2 = 1


def write_explicit():
    global x2
    x2 = 2


write_explicit()
assert x2 == 2, 'explicit global write'


x3 = 42


def read_implicit():
    return x3  # no local x3, reads global


assert read_implicit() == 42, 'implicit global read'


# === Multiple functions sharing global ===
counter1 = 0


def inc():
    global counter1
    counter1 = counter1 + 1


def get_counter():
    return counter1


inc()
inc()
assert get_counter() == 2, 'multiple functions sharing global'


# === Mutating global containers (no 'global' needed) ===
data1 = {'a': 1}


def add_dict_entry():
    data1['b'] = 2


add_dict_entry()
assert data1 == {'a': 1, 'b': 2}, 'mutate global dict'


items1 = [1, 2]


def append_list_item():
    items1.append(3)


append_list_item()
assert items1 == [1, 2, 3], 'mutate global list append'


items2 = ['a', 'c']


def insert_list_item():
    items2.insert(1, 'b')


insert_list_item()
assert items2 == ['a', 'b', 'c'], 'mutate global list insert'


items3 = []


def build_list():
    items3.append(1)
    items3.append(2)
    items3.append(3)


build_list()
assert items3 == [1, 2, 3], 'mutate global list multiple'


# === Reassigning global containers (requires 'global') ===
items4 = [1, 2]


def replace_list():
    global items4
    items4 = [3, 4, 5]


replace_list()
assert items4 == [3, 4, 5], 'reassign global list'


# === Nested functions with global ===
x4 = 1


def outer_global():
    def inner():
        global x4
        x4 = 10

    inner()


outer_global()
assert x4 == 10, 'nested inner global write'


x5 = 42


def outer_read():
    def inner():
        return x5  # reads global

    return inner()


assert outer_read() == 42, 'nested inner global read'


# === Shadowing ===
x6 = 10


def shadow_local():
    x6 = 20  # creates local (shadows global)
    return x6


assert shadow_local() == 20, 'local shadows global'


x7 = 10


def shadow_unchanged():
    x7 = 99  # local
    return x7


assert shadow_unchanged() == 99, 'shadowing returns local'
assert x7 == 10, 'global unchanged after shadowing'


# === `global X` for a name that doesn't yet exist at module level ===
# Regression: previously the prepare phase allocated a function-local slot for
# `ghost` here but tagged it `NameScope::Global`, so the compiler emitted a
# `LoadGlobal/StoreGlobal` with an operand that indexed past the module globals
# array — at best a NameError on an unrelated slot, at worst an OOB panic.
# Fixed by bubbling nested `global X` declarations up to the module name_map
# and emitting `LoadGlobalByName`/`StoreGlobalByName` for unresolved cases.


def declare_then_write():
    global ghost1
    ghost1 = 5


declare_then_write()
assert ghost1 == 5, 'global declaration then write makes name visible at module level'


def declare_then_read():
    global ghost2
    return ghost2


try:
    declare_then_read()
    raise AssertionError('expected NameError for never-assigned global')
except NameError as exc:
    assert str(exc) == "name 'ghost2' is not defined", 'NameError message for unassigned global'


# === Forward reference to a later module-level binding ===
# `late_value` is read inside `read_late_value` before being assigned at module
# level. The function-level reference compiles to `LoadGlobalByName`, which at
# runtime looks up the name in the module's name map — by the time the function
# runs, the later `late_value = 'bound'` has already allocated and populated a
# module slot for that name.


def read_late_value():
    return late_value


late_value = 'bound'
assert read_late_value() == 'bound', 'function sees later module-level binding'


# === Late binding overrides parse-time builtin resolution ===
# Both calls must work as written. The first call uses the builtin (via the
# runtime `builtin_for_name` fallback when the module slot is undefined). The
# `def min` between the calls overwrites the module slot, so the second call
# picks up the user-defined version.


def call_min():
    return min([3, 1, 2])


assert call_min() == 1, 'first call resolves to builtin min'


def min(*args):
    return 'shadowed'


assert call_min() == 'shadowed', 'second call resolves to user-defined min'


# === Module-scope late binding (script form of the REPL case) ===
# Parse-time builtin substitution at module scope must not fire for a name
# the script has already bound earlier. Once `def max` lands in name_map,
# subsequent `max(...)` references at module scope go through the slot, not
# the baked-in builtin.

assert max(1, 2) == 2, 'pre-binding: module-scope max resolves to builtin'


def max(*args):
    return 'shadowed-max'


assert max(1, 2) == 'shadowed-max', 'post-binding: module-scope max sees user-defined version'


# === `global` declared via lambda → nested function ===
# A lambda can't itself declare `global`, but it can call a function that does.
# The bubble-up mechanism in `prepare_function_def` materializes the inner
# function's `global ghost3` decl all the way to the module name_map.

ghost3 = 1


def lambda_set(v):
    (lambda x: __set_ghost3(x))(v)


def __set_ghost3(v):
    global ghost3
    ghost3 = v


lambda_set(99)
assert ghost3 == 99, '`global X` inside a function reachable via lambda still rebinds the module slot'


# === Deeply nested `global X` — bubble-up across multiple function levels ===
# The `global x` declaration inside `inner` is two scopes away from the module
# (outer → inner). The bubble-up mechanism must propagate it through both
# levels so the module slot for `x` exists when `inner` runs.


def deep_outer():
    def deep_inner():
        global deep_x
        deep_x = 'reached-module'

    deep_inner()


deep_outer()
assert deep_x == 'reached-module', 'global X bubbles up from doubly-nested function'
