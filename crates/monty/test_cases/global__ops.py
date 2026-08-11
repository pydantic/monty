# === Basic global read/write ===
x1 = 42


def read_explicit():
    global x1
    return x1


assert read_explicit() == 42


x2 = 1


def write_explicit():
    global x2
    x2 = 2


write_explicit()
assert x2 == 2


x3 = 42


def read_implicit():
    return x3  # no local x3, reads global


assert read_implicit() == 42


# === Multiple functions sharing global ===
counter1 = 0


def inc():
    global counter1
    counter1 = counter1 + 1


def get_counter():
    return counter1


inc()
inc()
assert get_counter() == 2


# === Mutating global containers (no 'global' needed) ===
data1 = {'a': 1}


def add_dict_entry():
    data1['b'] = 2


add_dict_entry()
assert data1 == {'a': 1, 'b': 2}


items1 = [1, 2]


def append_list_item():
    items1.append(3)


append_list_item()
assert items1 == [1, 2, 3]


items2 = ['a', 'c']


def insert_list_item():
    items2.insert(1, 'b')


insert_list_item()
assert items2 == ['a', 'b', 'c']


items3 = []


def build_list():
    items3.append(1)
    items3.append(2)
    items3.append(3)


build_list()
assert items3 == [1, 2, 3]


# === Reassigning global containers (requires 'global') ===
items4 = [1, 2]


def replace_list():
    global items4
    items4 = [3, 4, 5]


replace_list()
assert items4 == [3, 4, 5]


# === Nested functions with global ===
x4 = 1


def outer_global():
    def inner():
        global x4
        x4 = 10

    inner()


outer_global()
assert x4 == 10


x5 = 42


def outer_read():
    def inner():
        return x5  # reads global

    return inner()


assert outer_read() == 42


# === Shadowing ===
x6 = 10


def shadow_local():
    x6 = 20  # creates local (shadows global)
    return x6


assert shadow_local() == 20


x7 = 10


def shadow_unchanged():
    x7 = 99  # local
    return x7


assert shadow_unchanged() == 99
assert x7 == 10


# === `global X` for a name that doesn't yet exist at module level ===


def declare_then_write():
    global ghost1
    ghost1 = 5


declare_then_write()
assert ghost1 == 5


def declare_then_read():
    global ghost2
    return ghost2


try:
    declare_then_read()
    raise AssertionError('expected NameError for never-assigned global')
except NameError as exc:
    assert str(exc) == "name 'ghost2' is not defined"


# === Forward reference to a later module-level binding ===


def read_late_value():
    return late_value


late_value = 'bound'
assert read_late_value() == 'bound'


# === Late binding overrides parse-time builtin resolution ===


def call_min():
    return min([3, 1, 2])


assert call_min() == 1


def min(*args):
    return 'shadowed'


assert call_min() == 'shadowed'


# === Module-scope shadowing builtins ===

assert max(1, 2) == 2


def max(*args):
    return 'shadowed-max'


assert max(1, 2) == 'shadowed-max'


# === Deeply nested `global X` ===


def deep_outer():
    def deep_inner():
        global deep_x
        deep_x = 'reached-module'

    deep_inner()


deep_outer()
assert deep_x == 'reached-module'
