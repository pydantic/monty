import functools
import sys


# === partial ===
def target(a, b=1, *args, **kwargs):
    return (a, b, args, kwargs)


bound = functools.partial(target, 1)
assert bound() == (1, 1, (), {})
assert bound(2) == (1, 2, (), {})
assert bound(2, 3, c=4) == (1, 2, (3,), {'c': 4})

# bound positionals come before the call's own
assert functools.partial(target, 1, 2, 3)(4) == (1, 2, (3, 4), {})

# call keywords replace bound ones
kw = functools.partial(target, 1, b=2)
assert kw() == (1, 2, (), {})
assert kw(b=9) == (1, 9, (), {})
assert kw(c=3) == (1, 2, (), {'c': 3})

# a bound keyword that the call also fills positionally is an error, as it
# would be with the arguments written out
try:
    kw(3)
    assert False, 'expected the call to fail'
except TypeError as exc:
    assert str(exc) == "target() got multiple values for argument 'b'"

# === partial attributes ===
assert bound.func is target
assert bound.args == (1,)
assert bound.keywords == {}
assert kw.args == (1,)
assert kw.keywords == {'b': 2}
assert functools.partial(target, 1, 2, x=3, y=4).keywords == {'x': 3, 'y': 4}

try:
    kw.bogus
    assert False, 'expected the attribute lookup to fail'
except AttributeError as exc:
    assert str(exc) == "'functools.partial' object has no attribute 'bogus'"

# === partial nesting ===
# a partial wrapping a partial is flattened at construction
inner = functools.partial(target, 1, b=2)
outer = functools.partial(inner, 3, b=4)
assert outer.func is target
assert outer.args == (1, 3)
assert outer.keywords == {'b': 4}
assert functools.partial(functools.partial(target, 1)).args == (1,)
assert functools.partial(functools.partial(target, 1), 2)() == (1, 2, (), {})

# === partial identity and truthiness ===
assert type(bound) is functools.partial
assert isinstance(bound, functools.partial)
assert bool(functools.partial(target, 1))
# partials compare by identity, so two equivalent ones are not equal
assert functools.partial(target, 1) != functools.partial(target, 1)
assert bound == bound
# ... and hash by identity too, so equivalent partials are distinct keys
assert len({bound, kw, bound}) == 2
assert {bound: 'a'}[bound] == 'a'
assert len({functools.partial(target, 1), functools.partial(target, 1)}) == 2

# === partial repr ===
assert repr(functools.partial(int, '10')) == "functools.partial(<class 'int'>, '10')"
assert repr(functools.partial(int)) == "functools.partial(<class 'int'>)"
assert repr(functools.partial(int, '10', base=8)) == "functools.partial(<class 'int'>, '10', base=8)"

# a bound argument holding the partial back reprs as `...` rather than recursing
items = []
items.append(functools.partial(int, items))
assert repr(items[0]) == "functools.partial(<class 'int'>, [...])"
assert repr(items) == "[functools.partial(<class 'int'>, [...])]"
mapping = {}
mapping['self'] = functools.partial(int, k=mapping)
assert repr(mapping['self']) == "functools.partial(<class 'int'>, k={'self': ...})"

# === partial as an ordinary callable ===
assert list(map(functools.partial(pow, 2), [1, 2, 3])) == [2, 4, 8]
assert sorted([(1, 'b'), (2, 'a')], key=functools.partial(lambda i, t: t[i], 1)) == [(2, 'a'), (1, 'b')]
assert functools.reduce(functools.partial(lambda scale, a, b: scale * (a + b), 2), [1, 2, 3]) == 18


# === partial as a descriptor ===
# one stored on a class binds the instance after the arguments it already
# carries, as it does in CPython 3.14
class Holder:
    prefixed = functools.partial(target, 'p')

    def __init__(self):
        self.own = functools.partial(target, 'o')


holder = Holder()
assert holder.prefixed() == ('p', holder, (), {})
assert holder.prefixed(9) == ('p', holder, (9,), {})
assert getattr(holder, 'prefixed')() == ('p', holder, (), {})
# reached through the class there is no instance to bind
assert Holder.prefixed() == ('p', 1, (), {})
assert Holder.prefixed.args == ('p',)
# only a class attribute is a descriptor, so an instance one binds nothing
assert holder.own() == ('o', 1, (), {})

# === partial over builtins ===
# builtins reached through a partial must see the same argument shapes as a
# direct call does, whatever their arity
assert functools.partial(len)([1, 2, 3]) == 3
assert functools.partial(abs)(-4) == 4
assert functools.partial(list)() == []
assert functools.partial(list)('ab') == ['a', 'b']
assert functools.partial(isinstance)(1, int)
assert functools.partial(int, '10')() == 10
assert functools.partial(max, 1)(2) == 2
assert functools.partial(round)(1.234, 2) == 1.23

# === partial construction errors ===
try:
    functools.partial()
    assert False, 'expected partial to fail'
except TypeError as exc:
    assert str(exc) == "type 'partial' takes at least one argument"

try:
    functools.partial(func=target)
    assert False, 'expected partial to fail'
except TypeError as exc:
    assert str(exc) == "type 'partial' takes at least one argument"

try:
    functools.partial(5)
    assert False, 'expected partial to fail'
except TypeError as exc:
    assert str(exc) == 'the first argument must be callable'


# === deep partial chains are bounded, not a stack overflow ===
# A partial stored as a class attribute binds as a bound method whose `__func__`
# is another partial, so calling the outermost one nests on the interpreter's own
# call stack without pushing a Python frame. Monty caps that at a fixed native
# re-entry depth; CPython runs until its C stack gives out (see
# ./limitations/functools.md).
def chain(depth):
    def count(*args):
        return len(args)

    cur = count
    for _ in range(depth):

        class Holder:
            bound = functools.partial(cur)

        cur = Holder().bound
    return cur


assert chain(5)() == 5

if sys.platform == 'monty':
    try:
        chain(30)()
        assert False, 'expected the deep partial chain to raise'
    except RecursionError as exc:
        assert str(exc) == 'maximum recursion depth exceeded'
else:
    assert chain(30)() == 30
