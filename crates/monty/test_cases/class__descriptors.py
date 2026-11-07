import dataclasses

# === Descriptor notification and mutation ===


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


# === Read precedence ===


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
except AttributeError as error:
    assert str(error) == 'read only'
else:
    assert False, 'expected AttributeError'


# === Builtin wrappers ===


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
assert saved_class.__func__ is Wrapped.create.__func__
assert saved_class.__self__ is Wrapped
assert saved_class.__func__(Wrapped, 10).value == 10
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
except AttributeError as error:
    assert str(error) == "property '<lambda>' of 'Mutable' object has no setter"
else:
    assert False, 'expected AttributeError'
Mutable.x = NonData()
assert m.x == 10


# Setter-only and deleter-only descriptors affect writes without changing reads.
# === Mutation-only descriptors ===


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
except AttributeError as error:
    assert str(error) == '__delete__'
else:
    assert False, 'expected AttributeError'
try:
    wr.delete = 1
except AttributeError as error:
    assert str(error) == '__set__'
else:
    assert False, 'expected AttributeError'
del wr.delete
assert wr.deleted


# The result of __get__ is called without binding an additional receiver.
# === Calling descriptor results ===


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
# === Special method binding ===


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
# === Name notification order ===


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


# === ordinary attribute fast paths observe descriptor mutations ===


class FastPathRecord:
    x = 7
    __match_args__ = ('x',)


record = FastPathRecord()
assert record.x == 7
assert getattr(record, 'x', 0) == 7
assert hasattr(record, 'x')
assert not hasattr(record, 'absent')
match record:
    case FastPathRecord(x):
        assert x == 7

FastPathRecord.x = property(lambda obj: 42)
assert record.x == 42
assert getattr(record, 'x', 0) == 42
assert hasattr(record, 'x')
match record:
    case FastPathRecord(x):
        assert x == 42


class Candidate:
    pass


class LateRecord:
    y = Candidate()


class Inserted:
    pass


late = LateRecord()
inserted = Inserted()
late.y = 3
inserted.y = 4
Inserted.y = Candidate()
Candidate.__get__ = lambda self, obj, owner: 8
assert late.y == 3
assert inserted.y == 4


def store(self, obj, value):
    obj.stored = value


Candidate.__set__ = store
late.y = 9
inserted.y = 10
assert late.stored == 9
assert inserted.stored == 10
assert late.y == 8
assert inserted.y == 8
del Candidate.__set__
assert late.y == 3
assert inserted.y == 4


class FastPathNames:
    def __get__(self, obj, owner):
        assert obj is None
        return ('y',)


FastPathRecord.__match_args__ = FastPathNames()
record.y = 3
match record:
    case FastPathRecord(y):
        assert y == 3


# === mixed classes preserve lookup precedence and mutation ===


class MixedNonData:
    def __get__(self, obj, owner):
        return 91


class MixedRecord:
    __match_args__ = ('x',)
    candidate = MixedNonData()

    @property
    def computed(self):
        return 42

    @staticmethod
    def static():
        return 12

    @classmethod
    def class_method(cls):
        return 13

    def method(self):
        return self.x

    def defaults(self, delta=2):
        return self.x + delta


record = MixedRecord()
record.x = 7
record.candidate = 8
record.static = lambda: 22
record.class_method = lambda: 23
for _ in range(3):
    assert record.x == getattr(record, 'x') == 7
    assert record.method() == 7
    assert record.defaults() == 9
    assert record.defaults(delta=3) == 10
    assert getattr(record, 'method')() == 7
    assert record.static() == 22
    assert record.class_method() == 23
    assert record.computed == 42
    assert record.candidate == 8
    assert hasattr(record, 'x')
    assert not hasattr(record, 'absent')
    assert getattr(record, 'absent', 99) == 99
    match record:
        case MixedRecord(x):
            assert x == 7
MixedNonData.__set__ = lambda self, obj, value: None
assert record.candidate == 91
del MixedNonData.__set__
assert record.candidate == 8
MixedNonData.__delete__ = lambda self, obj: None
assert record.candidate == 91
del MixedNonData.__delete__
assert record.candidate == 8
MixedRecord.x = property(lambda obj: 17)
assert record.x == 17
del MixedRecord.x
assert record.x == 7
del record.static
del record.class_method
assert record.static() == 12
assert record.class_method() == 13
record.method = lambda: 31
assert record.method() == 31
del record.method
assert record.method() == 7
MixedRecord.method = property(lambda obj: lambda: 32)
assert record.method() == 32
MixedRecord.method = lambda self: self.x + 1
assert record.method() == 8


class OnlyNonData:
    static = staticmethod(lambda: 1)
    class_method = classmethod(lambda cls: 2)


other = OnlyNonData()
other.static = lambda: 3
other.class_method = lambda: 4
assert other.static() == 3
assert other.class_method() == 4


# === builtin descriptor get requires a receiver or owner ===

for constructor in (property, classmethod, staticmethod):
    descriptor = constructor(len)
    for args in ((None,), (None, None)):
        try:
            descriptor.__get__(*args)
        except TypeError as exc:
            assert str(exc) == '__get__(None, None) is invalid'
        else:
            assert False
    assert descriptor.__get__(None, list) is not None
42


# === native descriptor reads propagate accessor attribute errors ===

import dataclasses


@dataclasses.dataclass
class AccessorRecord:
    x: int


left = AccessorRecord(1)
right = AccessorRecord(1)
AccessorRecord.x = property()
for operation in (lambda: repr(left), lambda: left == right):
    try:
        operation()
    except AttributeError as exc:
        assert str(exc) == "property of 'AccessorRecord' object has no getter"
    else:
        assert False
42


# === delattr errors identify the class and invalid name type ===


class DeletionRecord:
    pass


try:
    delattr(DeletionRecord, 'missing')
except AttributeError as exc:
    assert str(exc) == "type object 'DeletionRecord' has no attribute 'missing'"
else:
    assert False

try:
    delattr(DeletionRecord(), 1)
except TypeError as exc:
    assert str(exc) == "attribute name must be string, not 'int'"
else:
    assert False


# === descriptor match args validation and short circuits ===


class PatternNames:
    def __get__(self, obj, owner):
        assert obj is None
        return owner.names


class PatternRecord:
    __match_args__ = PatternNames()
    names = ('x', 123)
    x = 42


# Unused tuple elements do not need to be strings.
match PatternRecord():
    case PatternRecord(x):
        assert x == 42

PatternRecord.names = []
# Neither a failed instance check nor a keyword-only pattern reads metadata.
match 42:
    case PatternRecord(x):
        assert False
match PatternRecord():
    case PatternRecord(x=x):
        assert x == 42

try:
    match PatternRecord():
        case PatternRecord(x):
            pass
except TypeError as e:
    assert str(e) == 'PatternRecord.__match_args__ must be a tuple (got list)'
else:
    assert False

PatternRecord.names = (123,)
try:
    match PatternRecord():
        case PatternRecord(x, y):
            pass
except TypeError as e:
    assert str(e) == 'PatternRecord() accepts 1 positional sub-pattern (2 given)'
else:
    assert False
try:
    match PatternRecord():
        case PatternRecord(x):
            pass
except TypeError as e:
    assert str(e) == '__match_args__ elements must be strings (got int)'
else:
    assert False


# === Exact wrapper errors and property names ===


class Accessors:
    empty = property()
    readonly = property(lambda self: 1)


accessors = Accessors()
for operation, expected in (
    (lambda: accessors.empty, "property 'empty' of 'Accessors' object has no getter"),
    (lambda: setattr(accessors, 'readonly', 1), "property 'readonly' of 'Accessors' object has no setter"),
    (lambda: delattr(accessors, 'readonly'), "property 'readonly' of 'Accessors' object has no deleter"),
    (lambda: Accessors.empty.__get__(accessors), "property 'empty' of 'Accessors' object has no getter"),
    (lambda: Accessors.empty.__set__(accessors, 1), "property 'empty' of 'Accessors' object has no setter"),
    (lambda: Accessors.empty.__delete__(accessors), "property 'empty' of 'Accessors' object has no deleter"),
    (lambda: property().__get__(accessors), "property of 'Accessors' object has no getter"),
    (lambda: property().__set__(accessors, 1), "property of 'Accessors' object has no setter"),
    (lambda: property().__delete__(accessors), "property of 'Accessors' object has no deleter"),
):
    try:
        operation()
        assert False, 'expected AttributeError'
    except AttributeError as error:
        assert str(error) == expected


for operation, expected in (
    (lambda: Accessors.empty.__get__(), '__get__ expected at least 1 argument, got 0'),
    (lambda: Accessors.empty.__get__(accessors, Accessors, None), '__get__ expected at most 2 arguments, got 3'),
    (lambda: Accessors.empty.__get__(obj=accessors), 'wrapper __get__() takes no keyword arguments'),
    (lambda: Accessors.empty.__set__(), '__set__ expected 2 arguments, got 0'),
    (lambda: Accessors.empty.__set__(accessors), '__set__ expected 2 arguments, got 1'),
    (lambda: Accessors.empty.__delete__(), 'expected 1 argument, got 0'),
    (lambda: Accessors.empty.__set_name__(), '__set_name__() takes 2 positional arguments but 0 were given'),
    (lambda: Accessors.empty.getter(), 'property.getter() takes exactly one argument (0 given)'),
    (lambda: Accessors.empty.setter(None, None), 'property.setter() takes exactly one argument (2 given)'),
    (lambda: Accessors.empty.deleter(fdel=None), 'property.deleter() takes no keyword arguments'),
):
    try:
        operation()
        assert False, 'expected TypeError'
    except TypeError as error:
        assert str(error) == expected


p = property()
p.__set_name__(Accessors, 42)
assert p.__name__ == 42
try:
    p.__get__(accessors)
    assert False, 'expected AttributeError'
except AttributeError as error:
    assert str(error) == "property 42 of 'Accessors' object has no getter"

# Names are owned Python values, so replacing one must release its previous reference.
for name in ('a long property name' * 10, [1, 2], None, 42):
    p.__set_name__(Accessors, name)
    assert p.__name__ is name
p.__set_name__(Accessors, [p])
assert p.__name__[0] is p


# Bound methods expose their function and receiver for both reads and direct calls.
class Bound:
    def method(self, x):
        return self, x

    @classmethod
    def make(cls, x):
        return cls, x


bound = Bound()
assert bound.method.__func__ is Bound.method
assert bound.method.__self__ is bound
assert bound.method.__func__(bound, 1) == (bound, 1)
assert Bound.make.__func__ is bound.make.__func__
assert Bound.make.__self__ is Bound
assert Bound.make.__func__(Bound, 2) == (Bound, 2)
