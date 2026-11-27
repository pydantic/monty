# === Builtin iterables, created lazily ===
events = []


def delegate(values):
    events.append(1)
    result = yield from values
    assert result is None
    yield 'end'


a = delegate([1, 2])
b = delegate(range(2))
assert events == []
assert next(a) == 1
assert next(b) == 0
assert next(a) == 2
assert next(b) == 1
assert next(a) == 'end'
assert next(b) == 'end'
assert next(a, None) is None
for values in [(), [], '', {}, range(0), set()]:
    assert next(delegate(values)) == 'end'
assert events == [1, 1, 1, 1, 1, 1, 1, 1]


def letters():
    yield from 'ab'
    yield from (3, 4)
    yield from {'k': 1}
    yield from map(str, [5])
    yield from filter(None, [0, 6])


assert list(letters()) == ['a', 'b', 3, 4, 'k', '5', 6]


def unreachable():
    if False:
        yield from [1]


assert next(unreachable(), 99) == 99

# === Return values travel up the delegation chain ===
shared = []


def inner():
    yield 1
    yield 2
    return shared


def middle(g):
    result = yield from g
    assert result is shared
    return result


def outer(g):
    result = yield from middle(g)
    assert result is shared
    yield 3
    assert (yield from g) is None


g = inner()
assert next(g) == 1
a = outer(g)
assert next(a) == 2
assert a.send(None) == 3
assert next(a, 99) == 99

# === A custom iterator's StopIteration value ===
shared = []


class Counter:
    def __init__(self):
        self.n = 0

    def __iter__(self):
        return self

    def __next__(self):
        self.n += 1
        if self.n == 3:
            raise StopIteration(shared)
        try:
            raise StopIteration('caught')
        except StopIteration:
            return self.n


def from_counter():
    result = yield from Counter()
    assert result is shared
    yield 42


assert list(from_counter()) == [1, 2, 42]


class Plain:
    def __iter__(self):
        return iter([7, 8])


def from_plain():
    result = yield from Plain()
    assert result is None
    yield 9


assert list(from_plain()) == [7, 8, 9]

# === send() is forwarded to a delegate generator ===


def accumulate():
    total = 0
    while True:
        value = yield total
        if value is None:
            return total
        total += value


def forwarder():
    result = yield from accumulate()
    yield ('result', result)


g = forwarder()
assert next(g) == 0
assert g.send(3) == 3
assert g.send(4) == 7
assert next(g) == ('result', 7)


def to_list():
    yield from [1, 2]


g = to_list()
assert next(g) == 1
try:
    g.send(5)
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'list_iterator' object has no attribute 'send'"

# === Exceptions propagate through delegation ===
events = []


def raises_later():
    yield 1
    raise ValueError('delegate')


def handles():
    try:
        yield from raises_later()
    except ValueError as exc:
        assert str(exc) == 'delegate'
        yield 2
    finally:
        events.append(3)


g = handles()
assert next(g) == 1
assert next(g) == 2
assert next(g, None) is None
assert events == [3]


def not_iterable():
    yield from 123


try:
    next(not_iterable())
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'int' object is not iterable"


def recursive():
    yield from g


g = recursive()
try:
    next(g)
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'generator already executing'
assert next(g, None) is None


def bad():
    raise StopIteration('bad')
    yield 1


def wrapper():
    yield from bad()


try:
    next(wrapper())
    assert False, 'expected RuntimeError'
except RuntimeError as exc:
    assert str(exc) == 'generator raised StopIteration'


class FailingIterator:
    def __iter__(self):
        return self

    def __next__(self):
        raise KeyError('next failed')


def from_failing():
    try:
        yield from FailingIterator()
    except KeyError as exc:
        yield str(exc)


assert list(from_failing()) == ["'next failed'"]

# === Deep delegation and the empty case ===


def chain(depth):
    if depth == 0:
        yield 'leaf'
        return 'bottom'
    return (yield from chain(depth - 1))


g = chain(20)
assert next(g) == 'leaf'
try:
    next(g)
    assert False, 'expected StopIteration'
except StopIteration as exc:
    assert exc.value == 'bottom'


def empties():
    for _ in range(3):
        yield from ()
    yield 'after'


assert list(empties()) == ['after']

# === send() reaches a delegate's own send method ===


class WithSend:
    def __init__(self):
        self.sent = []

    def __iter__(self):
        return self

    def __next__(self):
        return 'next'

    def send(self, value):
        self.sent.append(value)
        if value == 'stop':
            raise StopIteration('stopped')
        return ('sent', value)


def delegates_send(source):
    result = yield from source
    yield ('result', result)


source = WithSend()
g = delegates_send(source)
assert next(g) == 'next'
assert g.send(1) == ('sent', 1)
assert g.send('stop') == ('result', 'stopped')
assert source.sent == [1, 'stop']

# === A class as a delegate's __next__ or send ===
# Calling the class runs `__init__` as a frame; the new instance is the item.


class Item:
    def __init__(self):
        pass


class ClassNext:
    def __iter__(self):
        return self

    __next__ = Item


def spreads_class_items():
    yield [*(yield from ClassNext())]


g = spreads_class_items()
first = next(g)
second = next(g)
assert type(first) is Item and type(second) is Item
assert first is not second


class NewItem:
    def __new__(cls):
        return object.__new__(cls)

    def __init__(self):
        pass


class ClassNextWithNew:
    def __iter__(self):
        return self

    __next__ = NewItem


def spreads_new_items():
    yield {*(yield from ClassNextWithNew())}


g = spreads_new_items()
assert type(next(g)) is NewItem and type(next(g)) is NewItem


class ReturnsFive:
    def __init__(self):
        return 5


class BadInitNext:
    def __iter__(self):
        return self

    __next__ = ReturnsFive


def catches_init_error():
    try:
        yield from BadInitNext()
    except TypeError as e:
        yield str(e)


assert next(catches_init_error()) == "__init__() should return None, not 'int'"


class StopsInInit:
    def __init__(self):
        raise StopIteration('from init')


class StoppingNext:
    def __iter__(self):
        return self

    __next__ = StopsInInit


def delegation_result():
    result = yield from StoppingNext()
    yield ('result', result)


assert next(delegation_result()) == ('result', 'from init')


class Received:
    def __init__(self, value):
        self.value = value


class ClassSend:
    def __iter__(self):
        return self

    def __next__(self):
        return 0

    send = Received


def forwards_send():
    yield from ClassSend()


g = forwards_send()
assert next(g) == 0
received = g.send('hi')
assert type(received) is Received and received.value == 'hi'
