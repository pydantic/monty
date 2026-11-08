invalid_sources = [
    'return 1',
    'if False:\n    return 1',
    'def outer():\n    class C:\n        return 1',
    'def f(__debug__): pass',
    'obj.__debug__ = 1',
    'dict(__debug__=1)',
    '[(x := 1) for x in []]',
    '[x for x in (items := [])]',
    '[x for x in (lambda: (items := []))()]',
    '[[(x := y) for y in []] for x in []]',
    'class C:\n    x: [(value := x) for x in []]',
    'x: dict(a=1, a=2)',
    'try: pass\nexcept: pass\nexcept ValueError: pass',
    'def f(x):\n    global x',
    'def f():\n    x: int\n    global x',
    'def f[T, T](): pass',
    'def f[T: (value := 1)](): pass',
    'lambda: await value',
]
for source in invalid_sources:
    try:
        exec(source)
    except SyntaxError:
        pass
    else:
        assert False, source

for source in ['[(x := 1) for x in []]', '[x for x in (items := [])]', 'dict((x)=1)', 'dict(__debug__=1)']:
    try:
        eval(source)
    except SyntaxError:
        pass
    else:
        assert False, source

assert __debug__ is True
assert dict(**{'__debug__': 42}) == {'__debug__': 42}
assert eval('[(lambda: (x := 42))() for x in [0]]') == [42]
assert [(value := x) for x in [42]] == [42]
assert value == 42

exec('def returns_value():\n    return 42')
assert returns_value() == 42
