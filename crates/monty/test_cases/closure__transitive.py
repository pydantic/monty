# Multi-level (transitive / pass-through) closure capture: a nested function
# capturing a variable from a scope more than one level up. Each intermediate
# scope threads the captured cell through, matching CPython.


# === Two-level read ===
def outer_read(a):
    def mid():
        def inner():
            return a  # captured from `outer_read`, through `mid`

        return inner()

    return mid()


assert outer_read(10) == 10, 'inner reads grandparent param'


# === Four-level read ===
def a4(x):
    def b():
        def c():
            def d():
                return x * 2

            return d()

        return c()

    return b()


assert a4(21) == 42, 'capture four levels up'


# === Two-level nonlocal write ===
def writer():
    a = 0

    def mid():
        def inner():
            nonlocal a
            a += 5

        inner()

    mid()
    return a


assert writer() == 5, 'nonlocal write reaches grandparent local'


# === Intermediate scope rebinds the name (shadowing) ===
def shadow(x):
    def mid():
        x = 99  # mid's own local shadows the outer `x`

        def inner():
            return x  # captures mid's x, NOT outer's

        return inner()

    return (mid(), x)


assert shadow(1) == (99, 1), 'intermediate binding shadows the deeper capture'


# === Owner reads its own variable before the capturing def appears ===
# This pins the fix: the variable must be recognised as a cell up front, even
# though it is only captured by a grand-nested function further down.
def early_use(n):
    total = n  # read/assigned here, before `mid`/`inner` are defined
    total += 1

    def mid():
        def inner():
            return total  # captures `total` two levels up

        return inner()

    return (mid(), total)


assert early_use(10) == (11, 11), 'owner-side references stay consistent with capture'


# === Sibling closures two levels down share the same cell ===
def shared():
    v = 0

    def mid():
        def setter(x):
            nonlocal v
            v = x

        def getter():
            return v

        return setter, getter

    return mid()


s, g = shared()
s(42)
assert g() == 42, 'sibling closures share one cell across two levels'


# === Each instantiation captures its own cell ===
def make_adder(n):
    def mid():
        def add(x):
            return x + n

        return add

    return mid()


add3 = make_adder(3)
add5 = make_adder(5)
assert add3(10) == 13, 'first closure captures n=3'
assert add5(10) == 15, 'second closure captures n=5'
assert add3(10) == 13, 'independent closures do not interfere'


# === Comprehension nested two levels deep captures an enclosing variable ===
def comp(n):
    def mid():
        return [n + i for i in range(3)]

    return mid()


assert comp(10) == [10, 11, 12], 'comprehension captures grandparent variable'


# === Mixed capture from two different levels at once ===
def two_levels(a):
    def mid(b):
        def inner():
            return a + b  # `a` from outer (2 levels), `b` from mid (1 level)

        return inner()

    return mid(5)


assert two_levels(10) == 15, 'capture from two different enclosing levels'


# === Lambda capturing two levels up ===
def lam(a):
    def mid():
        return (lambda: a)()

    return mid()


assert lam(7) == 7, 'lambda captures grandparent variable'


# === Three-level nonlocal write ===
def writer3():
    a = 0

    def m1():
        def m2():
            def inner():
                nonlocal a
                a += 7

            inner()

        m2()

    m1()
    return a


assert writer3() == 7, 'nonlocal write reaches three levels up'


# === Chained nonlocal: each intermediate scope also declares it ===
def chained():
    a = 1

    def m1():
        nonlocal a

        def inner():
            nonlocal a
            a = 99

        inner()

    m1()
    return a


assert chained() == 99, 'nonlocal redeclared at each level'


# === Capturing function defined inside control-flow blocks ===
# Exercises the transitive cell-var pre-pass recursing through if/for bodies.
def control_flow(a, flag):
    if flag:

        def mid():
            for _ in range(1):

                def inner():
                    return a

                return inner()

        return mid()
    return -1


assert control_flow(5, True) == 5, 'capture through nested if/for defs'


# === Owner mutates the captured variable after building the closure ===
# The closure must observe the later value (a shared cell, not a snapshot).
def late_mutation():
    x = 1

    def mid():
        def inner():
            return x

        return inner

    f = mid()
    x = 42  # mutated after the closure was created
    return f()


assert late_mutation() == 42, 'closure sees post-creation mutation via the cell'


# === Default argument in a nested function references a transitive capture ===
def with_default(a):
    def mid():
        def inner(y=a):  # default evaluated in mid; `a` captured from outer
            return y

        return inner()

    return mid()


assert with_default(11) == 11, 'default argument uses a transitively captured variable'


# === Owner reads the var BEFORE a nested default captures it ===
# Pins the pre-pass bug: a transitive capture hidden in a nested function's
# default expression must promote the variable to a cell up front, otherwise the
# earlier `a + 1` read resolves it as a plain local and diverges at runtime.
def default_after_read(a):
    y = a + 1  # owner reads `a` before `mid`/`inner` are defined

    def mid():
        def inner(b=a):  # default evaluated in mid; `a` captured from outer
            return b

        return inner()

    return y, mid()


assert default_after_read(10) == (11, 10), 'default capture stays consistent with earlier owner read'


# === Single-level default capture with an earlier owner read ===
def default_one_level(a):
    y = a + 1

    def mid(x=a):  # default evaluated in outer; `a` captured one level up
        return x

    return y, mid()


assert default_one_level(10) == (11, 10), 'one-level default capture consistent with earlier read'


# === `def f(a=a)` gotcha: default RHS captures the enclosing name ===
# The right-hand `a` is evaluated in the enclosing scope (capturing the param),
# even though the nested function also has a parameter named `a`.
def same_name_default(a):
    def inner(a=a):  # RHS `a` is outer's param (10); param `a` shadows inside
        return a

    return inner()


assert same_name_default(10) == 10, 'default RHS captures the enclosing same-named variable'


# === Intermediate rebind shadows a default capture ===
# `mid` rebinds `a`, so `inner`'s default captures mid's `a`, not outer's.
def default_shadowed(a):
    def mid():
        a = 99

        def inner(b=a):  # captures mid's a (99), not outer's
            return b

        return inner()

    return (mid(), a)


assert default_shadowed(1) == (99, 1), 'intermediate rebind shadows a default capture'


# === `global` in an intermediate scope is not a capture candidate ===
# A name declared `global` is not a local binding, so a nested function reading
# it resolves to the module global rather than capturing a (non-existent) cell.
g_counter = 100


def uses_global():
    global g_counter
    g_counter = 20

    def inner():
        return g_counter  # the module global, not a captured cell

    return inner()


assert uses_global() == 20, 'nested read of a global-declared name hits the module global'
assert g_counter == 20, 'the global write is visible at module scope'


# The intermediate `global` even overrides an enclosing local of the same name.
g_shadowed = 100


def outer_with_global_mid():
    g_shadowed = 1  # outer local, shadowed for mid's chain by mid's `global`

    def mid():
        global g_shadowed

        def inner():
            return g_shadowed  # module global (100), not outer's local (1)

        return inner()

    return mid()


assert outer_with_global_mid() == 100, 'global in mid routes inner past the outer local'


# === Lambda whose default is a capturing lambda ===
# The default `(lambda: x)` is evaluated in the enclosing scope, so its `x`
# captures the enclosing `x` — NOT the outer lambda's same-named param. The
# cell-var pre-pass must therefore scan lambda defaults without filtering by the
# lambda's own params, else the enclosing cell is missed (the closure build then
# fails). `y` reads `x` first to pin that the owner stays consistent with the
# late-promoted cell.
def lambda_default_capture():
    x = 10
    y = x + 1
    g = lambda x=(lambda: x): x()
    return y, g()


assert lambda_default_capture() == (11, 10), 'lambda default captures enclosing same-named var'


# The same capture works two levels up and survives owner mutation after the
# inner closure is built (a shared cell, not a snapshot).
def lambda_default_two_level():
    v = 1

    def mid():
        g = lambda x=(lambda: v): x
        return g()

    inner = mid()
    v = 42  # mutated after the default closure was created
    return inner()


assert lambda_default_two_level() == 42, 'lambda default capture two levels up sees later mutation'
