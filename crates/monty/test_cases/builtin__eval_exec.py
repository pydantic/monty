# === eval of expressions ===
x = 10
assert eval('x + 1') == 11
assert eval('  \t 2 + 2') == 4
assert eval('\n3 * 3') == 9
assert eval(b'x * 2') == 20
assert eval('[i * 2 for i in range(3)]') == [0, 2, 4]
assert eval('eval("x")') == 10

# === exec binds module globals ===
exec('y = x * 2')
assert y == 20
assert exec('pass') is None
assert exec('') is None
exec(b'z = 1')
assert z == 1
exec('a1 = 1\nb1 = a1 + 1')
assert b1 == 2
exec('exec("nested = 7")')
assert nested == 7


# === builtins may be shadowed ===
def shadow():
    len = 3
    return eval('len')


assert shadow() == 3
assert eval('len([1, 2])') == 2


# === inside a function: locals are visible, exec writes are discarded ===
def f(a):
    b = a + 1
    exec('a = 100')
    return a, eval('a + b'), sorted(locals().keys())


assert f(1) == (1, 3, ['a', 'b'])


def g():
    v = 5
    return eval('[v for _ in range(2)]')


assert g() == [5, 5]


def h():
    n = 1
    inner = lambda: n
    return eval('n') + inner()


assert h() == 2


def k():
    exec('global gk\ngk = 3')


k()
assert gk == 3

# === eval in a sorted key ===
assert sorted(['b', 'a'], key=lambda s: eval('s')) == ['a', 'b']

# === exceptions from the snippet reach the caller ===
try:
    eval('1 / 0')
    assert False, 'expected ZeroDivisionError'
except ZeroDivisionError as e:
    assert str(e) == 'division by zero'

# === argument errors ===
try:
    eval(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'eval() arg 1 must be a string, bytes or code object'
try:
    exec(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'exec() arg 1 must be a string, bytes or code object'
try:
    eval('x', [])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'globals must be a real dict; try eval(expr, {}, mapping)'
try:
    eval('x', 1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'globals must be a dict'
try:
    exec('x', [])
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'exec() globals must be a dict, not list'
try:
    eval('x', {}, 3)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'locals must be a mapping'
try:
    exec('x', {}, 3)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'locals must be a mapping or None, not int'
try:
    exec('x', closure=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'closure can only be used when source is a code object'
try:
    eval()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'eval() takes at least 1 positional argument (0 given)'
try:
    eval('1', {}, {}, {})
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'eval() takes at most 3 arguments (4 given)'

# === syntax errors ===
try:
    eval('1\0')
    assert False, 'expected SyntaxError'
except SyntaxError as e:
    assert str(e) == 'source code string cannot contain null bytes'
try:
    exec('await x')
    assert False, 'expected SyntaxError'
except SyntaxError as e:
    assert str(e) == "'await' outside function (<string>, line 1)"
try:
    eval(b'\xff')
    assert False, 'expected SyntaxError'
except SyntaxError as e:
    assert str(e) == (
        "Non-UTF-8 code starting with '\\xff' on line 1, but no encoding declared; "
        'see https://peps.python.org/pep-0263/ for details (<string>, line 1)'
    )
# parse error wording comes from the parser, so only the location suffix is shared
try:
    eval('1 +')
    assert False, 'expected SyntaxError'
except SyntaxError as e:
    assert str(e).endswith('(<string>, line 1)')
# leading blank lines are skipped by eval but still counted in the line number
for source, line in [('\n)', 2), ('\n\n  )', 3), ('  \n\n*', 3)]:
    try:
        eval(source)
        assert False, 'expected SyntaxError'
    except SyntaxError as e:
        assert str(e).endswith(f'(<string>, line {line})')


# === recursion through eval is bounded ===
def deep(n):
    return eval('deep(n - 1)') if n else 0


try:
    deep(10_000)
    assert False, 'expected RecursionError'
except RecursionError:
    pass


# === locals() ===
def loc(a, b=2):
    c = a + b
    return list(locals().keys()), c


assert loc(1) == (['a', 'b', 'c'], 3)


def loc2():
    x = 1

    def inner():
        return x

    return list(locals().keys()), inner()


assert loc2() == (['inner', 'x'], 1)


def cap(x):
    f = lambda: x
    x = 2
    return list(locals().keys()), f()


assert cap(1) == (['x', 'f'], 2)
g2, l2 = {}, {}
exec('r = locals()', g2, l2)
assert l2['r'] is l2
assert eval('locals()', {'p': 1})['p'] == 1
