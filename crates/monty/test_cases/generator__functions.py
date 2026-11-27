# === Lazy creation and independent locals ===
events = []


def generate(start):
    events.append(start)
    received = yield start
    assert received is None
    yield start + 1


a = generate(1)
b = generate(10)
assert events == []
assert next(a) == 1
assert next(b) == 10
assert next(a) == 2
assert next(b) == 11
assert events == [1, 10]
assert next(a, 'end') == 'end'
assert next(a, None) is None
try:
    next(b)
    assert False, 'expected StopIteration'
except StopIteration as exc:
    assert exc.args == ()

# === The generator type ===
g = generate(0)
assert type(g).__name__ == 'generator'
assert iter(g) is g
assert repr(g).startswith('<generator object generate at 0x')
assert g == g
assert g != generate(0)
assert {g: 1}[g] == 1

# === Consumers ===


def count(n):
    for i in range(n):
        yield i


assert list(count(4)) == [0, 1, 2, 3]
assert tuple(count(2)) == (0, 1)
assert set(count(3)) == {0, 1, 2}
assert dict.fromkeys(count(2)) == {0: None, 1: None}
assert sum(count(5)) == 10
assert max(count(4)) == 3
assert min(count(4)) == 0
assert sorted(count(3), reverse=True) == [2, 1, 0]
assert any(count(3))
assert not all(count(3))
assert ','.join(str(x) for x in count(3)) == '0,1,2'
assert ''.join(c for c in 'ab') == 'ab'
assert list(map(str, count(2))) == ['0', '1']
assert list(zip(count(2), count(3))) == [(0, 0), (1, 1)]
assert list(enumerate(count(2))) == [(0, 0), (1, 1)]
assert [x * 2 for x in count(3)] == [0, 2, 4]
assert 2 in count(3)
assert 5 not in count(3)
x, y = count(2)
assert (x, y) == (0, 1)
first, *rest = count(4)
assert (first, rest) == (0, [1, 2, 3])
assert [*count(2), *count(2)] == [0, 1, 0, 1]

# === for loops, break and resumption ===
g = count(5)
for item in g:
    if item == 2:
        break
assert next(g) == 3
assert list(g) == [4]
assert list(g) == []
total = 0
for item in count(4):
    total += item
else:
    total += 100
assert total == 106

# === Exhaustion in a resumed exception handler leaves no active exception ===


def finishes_in_handler():
    try:
        raise ValueError('handled')
    except ValueError:
        yield 1
        return


g = finishes_in_handler()
assert next(g) == 1
for item in g:
    assert False, 'expected exhaustion'
try:
    raise
except RuntimeError as exc:
    assert str(exc) == 'No active exception to reraise'

# === Interleaved generators keep their own loop state ===
a = count(3)
b = count(3)
assert [(next(a), next(b)) for _ in range(3)] == [(0, 0), (1, 1), (2, 2)]

# === Unreachable yield still makes a generator ===
events = []


def unreachable():
    events.append(1)
    return
    yield 1


g = unreachable()
assert events == []
assert next(g, 'done') == 'done'
assert events == [1]


def ordinary():
    def nested():
        yield 1

    return 42


assert ordinary() == 42

# === return value becomes StopIteration.value ===
value = []


def returns_value():
    yield 1
    return value


g = returns_value()
assert next(g) == 1
try:
    next(g)
    assert False, 'expected StopIteration'
except StopIteration as exc:
    assert exc.value is value
    assert exc.args == (value,)
try:
    next(g)
    assert False, 'expected StopIteration'
except StopIteration as exc:
    assert exc.value is None
    assert exc.args == ()


def returns_string():
    return 'done'
    yield


try:
    next(returns_string())
    assert False, 'expected StopIteration'
except StopIteration as exc:
    assert exc.value == 'done'
    assert str(exc) == 'done'
    assert repr(exc) == "StopIteration('done')"


def returns_none():
    return
    yield


try:
    next(returns_none())
    assert False, 'expected StopIteration'
except StopIteration as exc:
    assert exc.value is None
    assert exc.args == ()

# === StopIteration values ===
assert StopIteration(42).value == 42
assert StopIteration().value is None
assert StopIteration(42).args == (42,)
assert repr(StopIteration(42)) == 'StopIteration(42)'
assert str(StopIteration(42)) == '42'
assert repr(StopIteration([1])) == 'StopIteration([1])'
assert repr(StopIteration()) == 'StopIteration()'
assert str(StopIteration('msg')) == 'msg'
items = [1]
assert StopIteration(items).value is items

# === Exceptions inside generators ===


def failure():
    yield 1
    raise ValueError('failed')


g = failure()
assert next(g) == 1
try:
    next(g)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'failed'
assert next(g, 'closed') == 'closed'


def caught_inside():
    try:
        yield 1
        raise KeyError('k')
    except KeyError:
        yield 'caught'
    yield 'after'


assert list(caught_inside()) == [1, 'caught', 'after']

# === A bare raise after a yield re-raises the handled exception ===


def reraise():
    try:
        raise ValueError('original')
    except ValueError:
        yield 1
        raise


g = reraise()
assert next(g) == 1
try:
    raise TypeError('caller')
except TypeError:
    try:
        next(g)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == 'original'
    try:
        raise
    except TypeError as exc:
        assert str(exc) == 'caller'
assert next(g, None) is None

# === StopIteration escaping a generator becomes RuntimeError (PEP 479) ===


def leaks_stop():
    yield 1
    next(iter([]))


g = leaks_stop()
assert next(g) == 1
try:
    next(g)
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'generator raised StopIteration'
    assert type(exc.__cause__) is StopIteration
assert next(g, None) is None


def raises_stop():
    raise StopIteration('bad')
    yield


try:
    next(raises_stop())
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'generator raised StopIteration'
    assert str(exc.__cause__) == 'bad'
    assert exc.__context__ is exc.__cause__
    assert exc.__suppress_context__
try:
    list(raises_stop())
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'generator raised StopIteration'


def catches_stop():
    try:
        next(iter([]))
    except StopIteration:
        yield 'handled'


assert list(catches_stop()) == ['handled']

# === Running a generator from inside itself ===


def reentrant():
    yield next(r)


r = reentrant()
try:
    next(r)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'generator already executing'
assert next(r, 'closed') == 'closed'

# === send ===


def echo():
    received = []
    value = yield 'start'
    while value is not None:
        received.append(value)
        value = yield value * 2
    return received


g = echo()
assert g.send(None) == 'start'
assert g.send(2) == 4
assert g.send(5) == 10
try:
    g.send(None)
    assert False, 'expected StopIteration'
except StopIteration as exc:
    assert exc.value == [2, 5]
try:
    echo().send(1)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "can't send non-None value to a just-started generator"
g = echo()
assert g.__next__() == 'start'
try:
    g.send()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'generator.send() takes exactly one argument (0 given)'

# Unbound methods through the type check their receiver
generator = type(echo())
g = echo()
assert generator.__next__(g) == 'start'
assert generator.send(g, 3) == 6
try:
    generator.send(1, None)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "descriptor 'send' for 'generator' objects doesn't apply to a 'int' object"

# === Yield expressions in operand positions ===
token = []


def operands():
    for i in range(3):
        result = ((yield token), [token, (yield token)], (yield token))
        assert result == (None, [token, None], None)
        yield i


g = operands()
for i in range(3):
    assert next(g) is token
    assert next(g) is token
    assert next(g) is token
    assert next(g) == i
assert next(g, 'done') == 'done'


def adds(offset):
    for i in range(3):
        result = 100 + (yield offset + i)
        yield result


g = adds(10)
assert next(g) == 10
try:
    next(g)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "unsupported operand type(s) for +: 'int' and 'NoneType'"
assert next(g, 'done') == 'done'
g = adds(10)
assert next(g) == 10
assert g.send(5) == 105

# === yield in try/finally keeps a pending return ===


def finally_yield():
    try:
        yield 1
        return [42]
    finally:
        yield 2


g = finally_yield()
assert next(g) == 1
assert next(g) == 2
try:
    next(g)
    assert False, 'expected StopIteration'
except StopIteration as exc:
    assert exc.value == [42]
    assert repr(exc) == 'StopIteration([42])'

# === yield inside with ===
log = []


class Manager:
    def __enter__(self):
        log.append('enter')
        return self

    def __exit__(self, *args):
        log.append('exit')


def in_with():
    with Manager():
        yield 1
        yield 2


assert list(in_with()) == [1, 2]
assert log == ['enter', 'exit']

# === Arguments, defaults, closures and lambdas ===
events = []


def keyword_only(a, *, b):
    events.append(a)
    yield a + b


for call in [lambda: keyword_only(), lambda: keyword_only(1), lambda: keyword_only(1, 2)]:
    try:
        call()
        assert False, 'expected TypeError'
    except TypeError:
        pass
assert events == []
assert next(keyword_only(1, b=2)) == 3


def factory(captured):
    def generate(start=1, *, step=2):
        yield captured + start
        yield captured + start + step

    return generate


assert list(factory(10)(step=3)) == [11, 14]


def counter_closure():
    total = 0

    def gen():
        nonlocal total
        while True:
            total += 1
            yield total

    return gen


g = counter_closure()()
assert [next(g), next(g), next(g)] == [1, 2, 3]
f = lambda x: (yield x)
assert next(f(42)) == 42
assert list(f(7)) == [7]


def variadic(*args, **kwargs):
    yield args
    yield kwargs


assert list(variadic(1, 2, k=3)) == [(1, 2), {'k': 3}]

# === Generators as methods ===


class Source:
    def __init__(self, value):
        self.value = value

    def __iter__(self):
        yield self.value
        yield self.value + 1

    def pairs(self):
        yield from ((self.value, i) for i in range(2))


assert [value for value in Source(41)] == [41, 42]
assert list(Source(1).pairs()) == [(1, 0), (1, 1)]


class Invalid:
    def __init__(self):
        yield 42


try:
    Invalid()
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "__init__() should return None, not 'generator'"

# === Recursive generators ===


def flatten(items):
    for item in items:
        if isinstance(item, list):
            yield from flatten(item)
        else:
            yield item


assert list(flatten([1, [2, [3, [4]], 5]])) == [1, 2, 3, 4, 5]


def tree(depth):
    if depth:
        for child in tree(depth - 1):
            yield child + 1
    yield 0


assert list(tree(3)) == [3, 2, 1, 0]

# === Generators passed to callbacks ===
g = count(1)
assert list(map(next, [g])) == [0]
assert list(map(lambda g: next(g, 'done'), [g])) == ['done']
assert sorted(count(3), key=lambda x: -x) == [2, 1, 0]
assert list(filter(None, count(3))) == [1, 2]

# === Infinite generators stop with their consumer ===


def naturals():
    n = 0
    while True:
        yield n
        n += 1


for n in naturals():
    if n == 3:
        break
assert n == 3
assert list(zip(naturals(), 'ab')) == [(0, 'a'), (1, 'b')]
