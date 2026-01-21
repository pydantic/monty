# === Basic lambda ===
# no-arg lambda
f = lambda: 42
assert f() == 42, 'no-arg lambda'

# single arg lambda
f = lambda x: x + 1
assert f(5) == 6, 'single arg lambda'

# === Multiple arguments ===
f = lambda x, y: x + y
assert f(2, 3) == 5, 'multi-arg lambda'

f = lambda x, y, z: x * y + z
assert f(2, 3, 4) == 10, 'three-arg lambda'

# === Default arguments ===
f = lambda x, y=10: x + y
assert f(5) == 15, 'lambda with default'
assert f(5, 3) == 8, 'lambda override default'

f = lambda x=1, y=2: x * y
assert f() == 2, 'lambda all defaults'
assert f(3) == 6, 'lambda override first default'
assert f(3, 4) == 12, 'lambda override all defaults'

# === Lambda as expression (immediate call) ===
assert (lambda x: x * 2)(5) == 10, 'immediate call'
assert (lambda: 'hello')() == 'hello', 'immediate call no args'
assert (lambda x, y: x - y)(10, 3) == 7, 'immediate call multi args'

# === Lambda in data structures ===
funcs = [lambda x: x + 1, lambda x: x * 2, lambda x: x**2]
assert funcs[0](3) == 4, 'lambda in list - add'
assert funcs[1](3) == 6, 'lambda in list - mul'
assert funcs[2](3) == 9, 'lambda in list - pow'

# === Lambda assigned and called later ===
square = lambda x: x * x
double = lambda x: x + x
assert square(4) == 16, 'lambda assigned square'
assert double(4) == 8, 'lambda assigned double'

# === Lambda with operations ===
f = lambda x: x > 5
assert f(6) is True, 'lambda comparison gt'
assert f(4) is False, 'lambda comparison not gt'

f = lambda x: x if x > 0 else -x
assert f(5) == 5, 'lambda ternary positive'
assert f(-5) == 5, 'lambda ternary negative'


# === Closures ===
def make_adder(n):
    return lambda x: x + n


add5 = make_adder(5)
add10 = make_adder(10)
assert add5(3) == 8, 'closure capture add5'
assert add10(3) == 13, 'closure capture add10'


def make_multiplier(factor):
    return lambda x: x * factor


times3 = make_multiplier(3)
assert times3(4) == 12, 'closure capture multiplier'

# === Nested lambdas ===
f = lambda x: lambda y: x + y
add_to_5 = f(5)
assert add_to_5(3) == 8, 'nested lambda'

f = lambda x: lambda y: lambda z: x + y + z
assert f(1)(2)(3) == 6, 'triple nested lambda'

# === Lambda in list comprehension ===
squared = [f(x) for x in [1, 2, 3, 4] for f in [lambda n: n * n]]
# Note: this tests lambda in comprehension context, though due to late binding
# all items use the same lambda

# === Lambda returns another lambda ===
compose = lambda f: lambda g: lambda x: f(g(x))
inc = lambda x: x + 1
double = lambda x: x * 2
inc_then_double = compose(double)(inc)
assert inc_then_double(3) == 8, 'lambda composition'  # double(inc(3)) = double(4) = 8

# === Lambda repr ===
f = lambda: None
r = repr(f)
assert '<lambda>' in r, 'lambda repr contains <lambda>'
assert 'function' in r, 'lambda repr contains function'

# === Lambda with *args ===
f = lambda *args: sum(args)
assert f() == 0, 'lambda varargs empty'
assert f(1) == 1, 'lambda varargs one'
assert f(1, 2, 3) == 6, 'lambda varargs multiple'

# === Lambda with keyword arguments ===
f = lambda x, *, y: x + y
assert f(1, y=2) == 3, 'lambda keyword only'

f = lambda **kwargs: len(kwargs)
assert f() == 0, 'lambda kwargs empty'
assert f(a=1, b=2) == 2, 'lambda kwargs multiple'

# === Mixed parameters ===
f = lambda a, b=2, *args, c, d=4, **kwargs: (a, b, args, c, d, len(kwargs))
result = f(1, 2, 3, 4, c=10, d=20, e=30, f=40)
assert result == (1, 2, (3, 4), 10, 20, 2), 'lambda mixed params'

# === Unpacking in immediate lambda calls ===
xs = [1, 2, 3]
assert (lambda *a: a)(*xs) == (1, 2, 3), 'lambda with *args unpacking'
assert (lambda **k: k)(**{'a': 1}) == {'a': 1}, 'lambda with **kwargs unpacking'
assert (lambda *a, **k: (a, k))(1, 2, x=3) == ((1, 2), {'x': 3}), 'lambda mixed unpack'

# === Lambda parameter shadowing ===
# Inner lambda shadows outer variable - outer should not capture it


def make_shadowing_lambda():
    x = 10
    # inner lambda has param x, so outer lambda should NOT capture x from make_shadowing_lambda
    return lambda: (lambda x: x + 1)


outer_fn = make_shadowing_lambda()
inner_fn = outer_fn()
assert inner_fn(5) == 6, 'inner lambda takes x as param'


def test_inner_lambda_capture():
    y = 5
    # outer lambda binds y as param, inner lambda captures from outer lambda, not test_inner_lambda_capture
    g = lambda y: (lambda: y)
    return g(7)()


assert test_inner_lambda_capture() == 7, 'inner lambda captures outer lambda param'
