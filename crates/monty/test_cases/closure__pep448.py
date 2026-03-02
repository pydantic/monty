# Tests for PEP 448 unpacking inside closures.
# These exercise the collect_*_from_expr helpers in prepare.rs which walk
# expressions to find walrus-operator assignments, cell variables, and
# referenced names in nested functions. Closures that reference variables
# used in PEP 448 positions (dict unpack, list/tuple/set literal, call args)
# are the only way to reach these code paths.

# === Closure capturing variable used in dict unpack ===
def outer_dict():
    d1 = {'a': 1, 'b': 2}
    d2 = {'c': 3}

    def inner():
        return {**d1, **d2}

    return inner()


assert outer_dict() == {'a': 1, 'b': 2, 'c': 3}, 'closure: dict unpack'


# === Closure capturing variable used in list unpack ===
def outer_list():
    items = [1, 2, 3]
    extra = [4, 5]

    def inner():
        return [*items, *extra]

    return inner()


assert outer_list() == [1, 2, 3, 4, 5], 'closure: list unpack'


# === Closure capturing variable used in tuple unpack ===
def outer_tuple():
    a = (1, 2)
    b = (3, 4)

    def inner():
        return (*a, *b)

    return inner()


assert outer_tuple() == (1, 2, 3, 4), 'closure: tuple unpack'


# === Closure capturing variable used in set unpack ===
def outer_set():
    items = [1, 2, 3]

    def inner():
        return {*items}

    return inner()


assert outer_set() == {1, 2, 3}, 'closure: set unpack'


# === Closure using PEP 448 in a function call (single * and **) ===
def outer_call_star():
    def f(*args, **kwargs):
        return (args, kwargs)

    args = [1, 2, 3]
    kw = {'x': 10}

    def inner():
        return f(*args, **kw)

    return inner()


assert outer_call_star() == ((1, 2, 3), {'x': 10}), 'closure: call *args **kw'


# === Closure using multiple * and ** in a call ===
def outer_multi():
    def f(*args, **kwargs):
        return (args, kwargs)

    a = [1, 2]
    b = [3, 4]
    kw1 = {'x': 10}
    kw2 = {'y': 20}

    def inner():
        return f(*a, *b, **kw1, **kw2)

    return inner()


assert outer_multi() == ((1, 2, 3, 4), {'x': 10, 'y': 20}), 'closure: multi-star call'
