# /// monty
# gc-interval = 1
# ///

from collections import defaultdict
from datetime import datetime


# === Cooperative lookup uses the receiver's MRO, not the defining class's bases ===
class Root:
    label = 'root'

    def __init__(self, value):
        super().__init__()
        self.value = value

    def order(self):
        return ['root']

    def read(self):
        return self.value


class Left(Root):
    label = 'left'

    def __init__(self, value):
        super().__init__(value + 1)

    def order(self):
        return ['left'] + super().order()

    def parent(self):
        return super()

    def captured_receiver(self, other, /):
        def capture():
            return self

        self = other
        make_proxy = super
        return make_proxy().read(), capture().value

    def nested(self):
        def read_parent(receiver):
            return super().read()

        return read_parent(self)

    def indirect(self, factory):
        assert __class__ is Left
        return factory(*()).read()

    def shadowed(self, super):
        return super()


class Right(Root):
    label = 'right'

    def __init__(self, value):
        super().__init__(value * 2)

    def order(self):
        return ['right'] + super().order()


class Diamond(Left, Right):
    def __new__(cls, value):
        return super().__new__(cls)


instance = Diamond(4)
instance.label = 'instance'
assert instance.value == 10
assert instance.order() == ['left', 'right', 'root']
assert instance.captured_receiver(Diamond(5)) == (12, 12)
assert instance.nested() == 10
assert instance.indirect(super) == 10
assert instance.shadowed(lambda: 'shadowed') == 'shadowed'
proxy = instance.parent()
assert type(proxy) is super
assert proxy.__class__ is super
assert proxy.__thisclass__ is Left
assert proxy.__self__ is instance
assert proxy.__self_class__ is Diamond
assert repr(proxy) == "<super: <class 'Left'>, <Diamond object>>"
assert proxy.label == 'right'
Right.label = 'new right'
assert proxy.label == 'new right'
read = proxy.read
assert read() == 10
assert super(Left, Diamond).read(instance) == 10
assert super(*[Right, instance]).label == 'root'
assert super(Left).__self__ is None
assert super(Left, None).__self_class__ is None


# === Native methods use the defining type's implementation ===
dt = datetime(2026, 8, 24, 12, 34)
assert super(datetime, dt).isoformat() == '2026-08-24'
assert super(datetime, datetime).isoformat(dt) == '2026-08-24'

# The inherited class method binds to defaultdict, not dict.
for receiver in (defaultdict, defaultdict()):
    result = super(defaultdict, receiver).fromkeys(['key'], 1)
    assert type(result) is defaultdict
    assert result == {'key': 1}


# === Class-bound super passes the most-derived class to the next hook ===
hooks = []


class First:
    def __init_subclass__(cls, **kwargs):
        hooks.append(('first', cls.__name__))
        super().__init_subclass__(**kwargs)


class Second:
    def __init_subclass__(cls, *, tag, **kwargs):
        hooks.append((tag, cls.__name__))
        super().__init_subclass__(**kwargs)


class Both(First, Second, tag='second'):
    pass


assert hooks == [('first', 'Both'), ('second', 'Both')]


# === A saved proxy retains its receiver, and can participate in a cycle ===
def discard_cycle():
    obj = Diamond(2)
    obj.proxy = obj.parent()
    return obj.proxy.read()


assert discard_cycle() == 6
assert Diamond(3).parent().read() == 8


# === Invalid calls ===
def no_class(receiver):
    return super()


class Invalid:
    def only_varargs(*args):
        return super()

    def local_class(self):
        __class__ = Invalid
        assert __class__ is Invalid
        return super()


def no_args():
    return super()


for call, message in (
    (no_args, 'super(): no arguments'),
    (Invalid().only_varargs, 'super(): no arguments'),
    (lambda: no_class(instance), 'super(): __class__ cell not found'),
    (Invalid().local_class, 'super(): __class__ cell not found'),
):
    try:
        call()
        assert False, 'expected RuntimeError'
    except RuntimeError as exc:
        assert str(exc) == message

for call, message in (
    (lambda: super(Left, instance, instance), 'super() expected at most 2 arguments, got 3'),
    (lambda: super(type=Left), 'super() takes no keyword arguments'),
    (lambda: super(1), 'super() argument 1 must be a type, not int'),
    (lambda: super(Left, Root), 'super(type, obj): obj (type Root) is not an instance or subtype of type (Left).'),
    (
        lambda: super(Left, Root(0)),
        'super(type, obj): obj (instance of Root) is not an instance or subtype of type (Left).',
    ),
):
    try:
        call()
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == message

try:
    proxy.missing(instance)
    assert False, 'expected AttributeError'
except AttributeError as exc:
    assert str(exc) == "'super' object has no attribute 'missing'"
