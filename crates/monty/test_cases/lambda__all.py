# === Basic lambda ===
# no-arg lambda
f = lambda: 42
assert f() == 42

# single arg lambda
f = lambda x: x + 1
assert f(5) == 6

# === Multiple arguments ===
f = lambda x, y: x + y
assert f(2, 3) == 5

f = lambda x, y, z: x * y + z
assert f(2, 3, 4) == 10

# === Default arguments ===
f = lambda x, y=10: x + y
assert f(5) == 15
assert f(5, 3) == 8

f = lambda x=1, y=2: x * y
assert f() == 2
assert f(3) == 6
assert f(3, 4) == 12

# === Lambda as expression (immediate call) ===
assert (lambda x: x * 2)(5) == 10
assert (lambda: 'hello')() == 'hello'
assert (lambda x, y: x - y)(10, 3) == 7

# === Lambda in data structures ===
funcs = [lambda x: x + 1, lambda x: x * 2, lambda x: x**2]
assert funcs[0](3) == 4
assert funcs[1](3) == 6
assert funcs[2](3) == 9

# === Lambda assigned and called later ===
square = lambda x: x * x
double = lambda x: x + x
assert square(4) == 16
assert double(4) == 8

# === Lambda with operations ===
f = lambda x: x > 5
assert f(6) is True
assert f(4) is False

f = lambda x: x if x > 0 else -x
assert f(5) == 5
assert f(-5) == 5


# === Closures ===
def make_adder(n):
    return lambda x: x + n


add5 = make_adder(5)
add10 = make_adder(10)
assert add5(3) == 8
assert add10(3) == 13


def make_multiplier(factor):
    return lambda x: x * factor


times3 = make_multiplier(3)
assert times3(4) == 12

# === Nested lambdas ===
f = lambda x: lambda y: x + y
add_to_5 = f(5)
assert add_to_5(3) == 8

f = lambda x: lambda y: lambda z: x + y + z
assert f(1)(2)(3) == 6

# === Lambda in list comprehension ===
squared = [f(x) for x in [1, 2, 3, 4] for f in [lambda n: n * n]]
# Note: this tests lambda in comprehension context, though due to late binding
# all items use the same lambda

# === Lambda returns another lambda ===
compose = lambda f: lambda g: lambda x: f(g(x))
inc = lambda x: x + 1
double = lambda x: x * 2
inc_then_double = compose(double)(inc)
assert inc_then_double(3) == 8  # double(inc(3)) = double(4) = 8

# === Lambda repr ===
f = lambda: None
r = repr(f)
assert '<lambda>' in r
assert 'function' in r

# === Lambda with *args ===
f = lambda *args: sum(args)
assert f() == 0
assert f(1) == 1
assert f(1, 2, 3) == 6

# === Lambda with keyword arguments ===
f = lambda x, *, y: x + y
assert f(1, y=2) == 3

f = lambda **kwargs: len(kwargs)
assert f() == 0
assert f(a=1, b=2) == 2

# === Mixed parameters ===
f = lambda a, b=2, *args, c, d=4, **kwargs: (a, b, args, c, d, len(kwargs))
result = f(1, 2, 3, 4, c=10, d=20, e=30, f=40)
assert result == (1, 2, (3, 4), 10, 20, 2)

# === Unpacking in immediate lambda calls ===
xs = [1, 2, 3]
assert (lambda *a: a)(*xs) == (1, 2, 3)
assert (lambda **k: k)(**{'a': 1}) == {'a': 1}
assert (lambda *a, **k: (a, k))(1, 2, x=3) == ((1, 2), {'x': 3})

# === Lambda parameter shadowing ===
# Inner lambda shadows outer variable - outer should not capture it


def make_shadowing_lambda():
    x = 10
    # inner lambda has param x, so outer lambda should NOT capture x from make_shadowing_lambda
    return lambda: lambda x: x + 1


outer_fn = make_shadowing_lambda()
inner_fn = outer_fn()
assert inner_fn(5) == 6


def test_inner_lambda_capture():
    y = 5
    # outer lambda binds y as param, inner lambda captures from outer lambda, not test_inner_lambda_capture
    g = lambda y: lambda: y
    return g(7)()


assert test_inner_lambda_capture() == 7


# === Lambda captures in control-flow test expressions ===
# Regression: the closure pre-scan must visit the test/iter expression of
# while/if/for, otherwise a lambda buried there is discovered after `x` has
# already been assigned a normal local slot. The late path then reuses that
# local slot as a cell slot, breaking the contiguous cell layout the VM
# assumes, and `LoadCell` indexes outside the frame's cell vector.


def while_test_capture():
    a = 0
    x = 1
    while False and (lambda: x)():
        pass
    return x


assert while_test_capture() == 1


def if_test_capture():
    a = 0
    x = 2
    if False and (lambda: x)():
        pass
    return x


assert if_test_capture() == 2


def for_iter_capture():
    a = 0
    x = 3
    for _ in [(lambda: x)()]:
        pass
    return x


assert for_iter_capture() == 3


# Same as above, but the lambda is actually invoked and must read the cell.
def while_test_capture_runs():
    a = 0
    x = 7
    count = 0
    while (lambda: x == 7)() and count < 2:
        count += 1
    return (x, count)


assert while_test_capture_runs() == (7, 2)


def for_iter_capture_runs():
    a = 0
    x = 5
    seen = []
    for v in [(lambda: x)()]:
        seen.append(v)
    return (x, seen)


assert for_iter_capture_runs() == (5, [5])
