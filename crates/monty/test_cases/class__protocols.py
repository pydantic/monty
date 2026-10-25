from collections import defaultdict
from copy import deepcopy
from functools import partial


# === Subscription passes keys and slices unchanged ===
class Container:
    def __init__(self):
        self.items = {}
        self.calls = []

    def __getitem__(self, key):
        self.calls.append(('get', key))
        return self.items[key]

    def __setitem__(self, key, value):
        self.calls.append(('set', key, value))
        self.items[key] = value
        return ['discarded return value', self]


container = Container()
get_item = container.__getitem__
set_item = container.__setitem__
container.__getitem__ = None
container.__setitem__ = None
value = ['stored']
container['key'] = value
assert container['key'] is value
assert container.calls == [('set', 'key', value), ('get', 'key')]
container[1, 2] = None
assert container[1, 2] is None
container['not implemented'] = NotImplemented
assert container['not implemented'] is NotImplemented


class SliceReceiver:
    def __getitem__(self, key):
        return key

    def __setitem__(self, key, value):
        self.key = key
        self.value = value


receiver = SliceReceiver()
key = receiver[1:5:2]
assert (key.start, key.stop, key.step) == (1, 5, 2)
receiver[1:5:2] = value
assert (receiver.key.start, receiver.key.stop, receiver.key.step) == (1, 5, 2)
assert receiver.value is value

try:
    container['missing']
    assert False, 'expected __getitem__ exception'
except KeyError as exc:
    assert str(exc) == "'missing'"


class BadSet:
    def __setitem__(self, key, value):
        raise ValueError('cannot assign')


try:
    BadSet()[['key']] = ['value']
    assert False, 'expected __setitem__ exception'
except ValueError as exc:
    assert str(exc) == 'cannot assign'


# === Callable instances use class-only lookup and normal argument binding ===
class Callable:
    def __call__(self, first, *args, flag=None, **kwargs):
        return (self, first, args, flag, kwargs)


callable_instance = Callable()
call_method = callable_instance.__call__
callable_instance.__call__ = None
assert callable_instance(1, 2, 3, flag=4, extra=5) == (callable_instance, 1, (2, 3), 4, {'extra': 5})
assert callable_instance(*[1, 2], **{'flag': 3}) == (callable_instance, 1, (2,), 3, {})
assert partial(callable_instance, 1)(flag=2) == (callable_instance, 1, (), 2, {})


class Counter:
    def __init__(self):
        self.count = 0

    def __call__(self):
        self.count += 1
        return self.count


counter = Counter()
assert list(iter(counter, 4)) == [1, 2, 3]
factory = defaultdict(Counter())
assert factory['a'] == 1
assert factory['b'] == 2
copied_factory = deepcopy(factory)
assert copied_factory['c'] == 3
copied_partial = deepcopy(partial(Counter()))
assert copied_partial() == 1


# === Builtin, bound-method and callable-instance class members remain unbound ===
class BoundCall:
    __call__ = call_method


assert BoundCall()(1) == (callable_instance, 1, (), None, {})


class BuiltinCall:
    __call__ = len


assert BuiltinCall()([1, 2]) == 2


class ForwardCall:
    __call__ = callable_instance


assert ForwardCall()(1) == (callable_instance, 1, (), None, {})


class BoundSubscript:
    __getitem__ = get_item
    __setitem__ = set_item


bound = BoundSubscript()
bound['bound'] = value
assert bound['bound'] is value
assert container['bound'] is value


# === Method exceptions and non-callable slots propagate unchanged ===
class Raises:
    def __call__(self, value):
        raise ValueError('call failed')


try:
    Raises()(['argument'])
    assert False, 'expected call exception'
except ValueError as exc:
    assert str(exc) == 'call failed'


class Disabled:
    __call__ = None
    __getitem__ = None
    __setitem__ = None
    __neg__ = None
    __pos__ = None


for operation in [lambda x: x(['value']), lambda x: x[['key']], lambda x: -x, lambda x: +x]:
    try:
        operation(Disabled())
        assert False, 'expected non-callable special method'
    except TypeError as exc:
        assert str(exc) == "'NoneType' object is not callable"
try:
    Disabled()[['key']] = ['value']
    assert False, 'expected non-callable __setitem__'
except TypeError as exc:
    assert str(exc) == "'NoneType' object is not callable"

# A present __call__, even None, satisfies the callable check in partial.
disabled_partial = partial(Disabled())
try:
    disabled_partial(['value'])
    assert False, 'expected partial to dispatch the non-callable slot'
except TypeError as exc:
    assert str(exc) == "'NoneType' object is not callable"


# === Instance-only special methods do not enable protocols ===
class Plain:
    pass


plain = Plain()
plain.__call__ = lambda: 1
plain.__getitem__ = lambda key: key
plain.__setitem__ = lambda key, value: None
plain.__neg__ = lambda: 1
plain.__pos__ = lambda: 1
for operation, message in [
    (lambda: plain(), "'Plain' object is not callable"),
    (lambda: plain['key'], "'Plain' object is not subscriptable"),
    (lambda: -plain, "bad operand type for unary -: 'Plain'"),
    (lambda: +plain, "bad operand type for unary +: 'Plain'"),
    (lambda: partial(plain), 'the first argument must be callable'),
]:
    try:
        operation()
        assert False, 'expected instance-only special method to be ignored'
    except TypeError as exc:
        assert str(exc) == message
try:
    plain[['key']] = ['value']
    assert False, 'expected instance-only __setitem__ to be ignored'
except TypeError as exc:
    assert str(exc) == "'Plain' object does not support item assignment"

# A class mutation becomes visible immediately, including to callable checks.
Plain.__call__ = lambda self: ['called']
assert plain() == ['called']
assert partial(plain)() == ['called']


# === Reentrant methods can mutate their receiver ===
class Reentrant:
    def __getitem__(self, key):
        if key:
            self[key - 1] = ['nested']
        return self.last

    def __setitem__(self, key, value):
        self.last = (key, value)


assert Reentrant()[3] == (2, ['nested'])
