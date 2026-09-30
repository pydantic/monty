# === eval of expressions ===
x = 10
assert eval('x + 1') == 11
assert eval('  \t 2 + 2') == 4
assert eval('\n3 * 3') == 9
assert eval(b'x * 2') == 20
assert eval('[i * 2 for i in range(3)]') == [0, 2, 4]
assert eval('eval("x")') == 10
assert eval('x + 1', None, None) == 11

# === exec binds module globals ===
exec('y = x * 2')
assert y == 20
assert exec('pass') is None
assert exec('pass', None, None, closure=None) is None
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
    exec('async def value():\n    return 42\nclass C:\n    x = await value()')
    assert False, 'expected SyntaxError'
except SyntaxError as e:
    assert str(e) == "'await' outside function (<string>, line 4)"
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


# === async functions and methods inside snippets still accept await ===
async_namespace = {}
exec(
    'async def value():\n    return 42\n'
    'async def f():\n    return await value()\n'
    'class C:\n    async def method(self):\n        return await value()',
    async_namespace,
)
assert type(async_namespace['f']).__name__ == 'function'
assert type(async_namespace['C'].method).__name__ == 'function'


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


# === Captures passed through to nested closures are still function locals ===
def passthrough_locals():
    first, second = 41, 1

    def middle():
        assert sorted(locals()) == ['first', 'second']

        def inner():
            return first + second

        assert eval('first + second') == 42
        seen = []
        exec('seen.append(first + second)')
        assert seen == [42]
        snapshot = locals()
        snapshot['first'] = 0
        return inner()

    return middle()


assert passthrough_locals() == 42


def passthrough_lambda():
    value = 7
    middle = lambda: (locals()['value'], lambda: value)
    return middle()


captured, inner = passthrough_lambda()
assert captured == 7
assert inner() == 7


def nonlocal_locals():
    value = 3

    def inner():
        nonlocal value
        return locals()['value']

    return inner()


assert nonlocal_locals() == 3


def unbound_passthrough():
    def middle():
        def inner():
            return value

        return sorted(locals())

    names = middle()
    value = 42
    return names


assert unbound_passthrough() == ['inner']

# === Compilation while a builtin borrows an interned receiver ===
compile_source = '\n'.join(
    [
        f'def generated_{i}():\n    return ("literal_{i}", b"literal_{i}", 123456789012345678901234567890 + {i})'
        for i in range(300)
    ]
)


class CompilingIterator:
    def __init__(self, item):
        self.item = item
        self.remaining = 2

    def __iter__(self):
        return self

    def __next__(self):
        if self.remaining == 0:
            raise StopIteration
        self.remaining -= 1
        exec(compile_source, {})
        return self.item


assert 'borrowed separator'.join(CompilingIterator('x')) == 'xborrowed separatorx'
assert b'borrowed separator'.join(CompilingIterator(b'x')) == b'xborrowed separatorx'
assert eval('"borrowed separator"') == 'borrowed separator'

# === A runtime error does not discard published definitions ===
retained_namespace = {}
try:
    exec('def retained():\n    return "published literal"\nraise ValueError("after definition")', retained_namespace)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'after definition'
assert retained_namespace['retained']() == 'published literal'
