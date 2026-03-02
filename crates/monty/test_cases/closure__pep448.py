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


# === Closure calling with keyword-only args (ArgExprs::Kwargs) ===
# The outer-function assignment `_precomp = f(a=val_a, b=val_b)` exercises
# collect_assigned_names_from_args for the Kwargs arm.  The inner body exercises
# collect_cell_vars_from_args and collect_referenced_names_from_args for Kwargs.
def outer_kwargs_call():
    def f(**kwargs):
        return kwargs

    val_a = 10
    val_b = 20
    # Assignment RHS is a Kwargs call → triggers collect_assigned_names_from_args Kwargs arm
    _precomp = f(a=val_a, b=val_b)

    def inner():
        # Kwargs call in inner body → collect_cell/referenced_names Kwargs arm
        return f(a=val_a, b=val_b)

    return inner()


assert outer_kwargs_call() == {'a': 10, 'b': 20}, 'closure: keyword-only call'


# === Closure calling with positional + *star (ArgsKargs with args=Some) ===
# Exercises ArgsKargs branch where the positional-args field is non-None.
def outer_argsstar():
    def f(*args):
        return list(args)

    pos_val = 1
    items = [2, 3]
    # Assignment RHS has positional arg + star → ArgsKargs with args=Some([pos_val])
    _precomp = f(pos_val, *items)

    def inner():
        return f(pos_val, *items)

    return inner()


assert outer_argsstar() == [1, 2, 3], 'closure: positional + *args'


# === Closure calling with named kwarg + **kw (ArgsKargs with kwargs=Some) ===
# Exercises ArgsKargs branch where the named-kwargs field is non-None.
def outer_kwargsstar():
    def f(**kwargs):
        return kwargs

    val = 1
    extra = {'b': 2}
    # Assignment RHS has named kwarg + double-star → ArgsKargs with kwargs=Some
    _precomp = f(a=val, **extra)

    def inner():
        return f(a=val, **extra)

    return inner()


assert outer_kwargsstar() == {'a': 1, 'b': 2}, 'closure: named kwarg + **kw'


# === Closure with GeneralizedCall and Named kwarg ===
# Exercises the CallKwarg::Named arm in all three collect_*_names_from_args functions.
def outer_generalized_named():
    def f(*args, **kwargs):
        return (args, kwargs)

    a = [1, 2]
    b = [3]
    key_val = 99
    # Assignment RHS has GeneralizedCall with Named kwarg → collect_assigned_names Named arm
    _precomp = f(*a, *b, key=key_val)

    def inner():
        # Named kwarg in GeneralizedCall → collect_cell/referenced_names Named arm
        return f(*a, *b, key=key_val)

    return inner()


assert outer_generalized_named() == ((1, 2, 3), {'key': 99}), 'closure: generalized call with named kwarg'


# === Closure with GeneralizedCall and plain Value arg ===
# Exercises the CallArg::Value path of the GeneralizedCall args loop,
# covering the Value branch of the `CallArg::Value | CallArg::Unpack` OR-pattern.
def outer_generalized_mixed():
    def f(*args):
        return list(args)

    const = 0
    items1 = [1, 2]
    items2 = [3, 4]
    # GeneralizedCall with Value(const) + Unpack(items1) + Unpack(items2)
    _precomp = f(const, *items1, *items2)

    def inner():
        return f(const, *items1, *items2)

    return inner()


assert outer_generalized_mixed() == [0, 1, 2, 3, 4], 'closure: generalized call with value + unpack args'
