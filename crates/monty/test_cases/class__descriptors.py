import dataclasses


class Field:
    def __set_name__(self, owner, name):
        self.name = '_' + name
        self.owner = owner

    def __get__(self, obj, owner=None):
        if obj is None:
            return self
        assert owner is self.owner
        return getattr(obj, self.name)

    def __set__(self, obj, value):
        setattr(obj, self.name, value * 2)
        return 100

    def __delete__(self, obj):
        delattr(obj, self.name)
        return 200


class Record:
    value = Field()


r = Record()
r.value = 4
assert r.value == 8
assert Record.value.name == '_value'
del r.value
assert not hasattr(r, 'value')
assert getattr(r, 'value', 99) == 99
setattr(r, 'value', 5)
assert getattr(r, 'value') == 10


class NonData:
    def __get__(self, obj, owner):
        return 11


class ReadOnly:
    def __get__(self, obj, owner):
        return 12

    def __set__(self, obj, value):
        raise AttributeError('read only')


class Other:
    x = NonData()
    y = ReadOnly()


o = Other()
assert o.x == 11
o.x = 20
assert o.x == 20
assert o.y == 12
try:
    o.y = 3
except AttributeError:
    pass
else:
    assert False


class Wrapped:
    n = 7

    def __init__(self):
        self._value = 1

    @property
    def value(self):
        return self._value

    @value.setter
    def value(self, value):
        self._value = value
        return 'ignored'

    @value.deleter
    def value(self):
        del self._value
        return 'ignored'

    @classmethod
    def create(cls, value=2):
        result = cls()
        result.value = value
        return result

    @staticmethod
    def add(a, b=3):
        return a + b


w = Wrapped.create(value=8)
assert w.value == 8
assert Wrapped.add(2, b=4) == 6
assert w.add(2) == 5
saved_class = w.create
saved_static = w.add
assert saved_class(9).value == 9
assert saved_static(3, 4) == 7
assert isinstance(Wrapped.value, property)
assert Wrapped.value.fset is not None
del w.value
assert not hasattr(w, 'value')
assert getattr(w, 'value', 'missing') == 'missing'
w.value = 18
assert w.value == 18


def decorate(function):
    def wrapped(*args, **kwargs):
        return function(*args, **kwargs) + 1

    return wrapped


class Decorated:
    @decorate
    def method(self, x):
        return x * 2


assert Decorated().method(3) == 7

# Hooks on the descriptor instance do not participate in protocol lookup.
d = NonData()
d.__get__ = lambda *args: 99
Dynamic = type('Dynamic', (), {'a': d})
assert Dynamic().a == 11

# Installing a descriptor after class creation does not call __set_name__.
late = Field()
Record.late = late
assert not hasattr(late, 'name')


# A data descriptor shadows an attribute already present in instance storage.
class Mutable:
    x = 1


m = Mutable()
m.x = 10
Mutable.x = property(lambda obj: 20)
assert m.x == 20
try:
    m.x = 30
except AttributeError:
    pass
else:
    assert False
Mutable.x = NonData()
assert m.x == 10


# Setter-only and deleter-only descriptors affect writes without changing reads.
class WriteOnly:
    def __set__(self, obj, value):
        obj.written = value


class DeleteOnly:
    def __delete__(self, obj):
        obj.deleted = True


class WriteRecord:
    write = WriteOnly()
    delete = DeleteOnly()


wr = WriteRecord()
wr.write = 5
assert wr.written == 5
assert wr.write is WriteRecord.write
try:
    del wr.write
except AttributeError:
    pass
else:
    assert False
try:
    wr.delete = 1
except AttributeError:
    pass
else:
    assert False
del wr.delete
assert wr.deleted


# The result of __get__ is called without binding an additional receiver.
class CallableDescriptor:
    def __get__(self, obj, owner):
        return lambda *args, **kwargs: (args, kwargs)


class Calls:
    f = CallableDescriptor()


calls = Calls()
assert calls.f(1, x=2) == ((1,), {'x': 2})
assert calls.f(*[1, 2], **{'x': 3}) == ((1, 2), {'x': 3})
assert Calls.f(3) == ((3,), {})
saved = calls.f
assert saved(4) == ((4,), {})


# Special methods use the same binding rules as ordinary attribute lookup.
class Special:
    @staticmethod
    def __init__(value):
        assert value == 4

    @classmethod
    def __call__(cls, value):
        return cls, value

    @staticmethod
    def __iter__():
        return iter([1, 2])


special = Special(4)
assert special(7) == (Special, 7)
assert list(special) == [1, 2]

# Explicit protocol calls can receive builtin values too.
length = property(len)
assert length.__get__([1, 2], list) == 2
assert length.__get__([1]) == 1
assert length.__get__(None, Calls) is length
static = staticmethod(lambda x: x + 1)
assert static(3) == 4
assert static.__get__(None, Calls)(4) == 5
cm = classmethod(lambda cls: cls)
assert cm.__get__(None, Calls)() is Calls
assert cm.__get__(calls)() is Calls
assert isinstance(cm, classmethod)
assert isinstance(static, staticmethod)


# Native and user __set_name__ hooks run together in namespace order.
class Observer:
    def __set_name__(self, owner, name):
        self.seen = owner.alias.__name__


class Notifications:
    before = Observer()
    alias = property(lambda obj: 1)
    after = Observer()


assert Notifications.before.seen == '<lambda>'
assert Notifications.after.seen == 'alias'
assert Notifications.alias.__name__ == 'alias'


# The generated dataclass initializer and object.__setattr__ honor setters.
@dataclasses.dataclass
class DataRecord:
    value: int


field = Field()
DataRecord.value = field
field.__set_name__(DataRecord, 'value')
data = DataRecord(3)
assert data.value == 6
assert repr(data) == 'DataRecord(value=6)'
assert data == DataRecord(3)
object.__setattr__(data, 'value', 4)
assert data.value == 8


@dataclasses.dataclass(frozen=True)
class FrozenRecord:
    value: int


frozen = FrozenRecord(3)
try:
    del frozen.value
except dataclasses.FrozenInstanceError as error:
    assert str(error) == "cannot delete field 'value'"
else:
    assert False


def replacement_name_hook(self, owner, name):
    self.seen = 'new'


class Target:
    def __set_name__(self, owner, name):
        self.seen = 'old'


class Mutator:
    def __set_name__(self, owner, name):
        Target.__set_name__ = replacement_name_hook


class ChangingHooks:
    first = Mutator()
    second = Target()


assert ChangingHooks.second.seen == 'new'
