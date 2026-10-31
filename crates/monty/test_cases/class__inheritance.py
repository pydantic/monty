# Test deep inheritance
class Grandparent:
    x = 1


class Parent(Grandparent):
    y = 2


class Child(Parent):
    z = 3


c = Child()
assert c.x == 1
assert c.y == 2
assert c.z == 3
assert isinstance(c, Child)
assert isinstance(c, Parent)
assert isinstance(c, Grandparent)


# Test multiple inheritance (Left-to-right C3 resolution)
class A:
    name = 'A'
    shared = 'from A'


class B:
    name = 'B'
    shared = 'from B'
    b_only = 'only B'


class C(A, B):
    name = 'C'


obj = C()
assert obj.name == 'C'  # From C
assert obj.shared == 'from A'  # Left-most base class wins
assert obj.b_only == 'only B'  # Found in B
assert isinstance(obj, C)
assert isinstance(obj, A)
assert isinstance(obj, B)


# Test diamond inheritance with C3 MRO
class DiamondBase:
    val = 'root'


class DiamondLeft(DiamondBase):
    pass


class DiamondRight(DiamondBase):
    val = 'right'


class DiamondBottom(DiamondLeft, DiamondRight):
    pass


d = DiamondBottom()
assert d.val == 'right'
assert isinstance(d, DiamondBottom)
assert isinstance(d, DiamondLeft)
assert isinstance(d, DiamondRight)
assert isinstance(d, DiamondBase)


# Test inheriting explicitly from object
class WithObject(object):
    greeting = 'hi'


wo = WithObject()
assert wo.greeting == 'hi'
assert isinstance(wo, WithObject)


# Test inheritance inside an enclosing function scope
def make_child():
    class LocalBase:
        x = 42

    class LocalChild(LocalBase):
        y = 100

    return LocalChild()


lc = make_child()
assert lc.x == 42
assert lc.y == 100


# Test the mock BaseModel pattern
class MockBaseModel:
    def __init__(self, **kwargs):
        for key, value in kwargs.items():
            setattr(self, key, value)


class Input(MockBaseModel):
    limit = 20


payload = {'limit': 50, 'extra': 'data'}
obj = Input(**payload)

assert obj.limit == 50
assert obj.extra == 'data'
assert isinstance(obj, Input)
assert isinstance(obj, MockBaseModel)


# Test duplicate base rejection
class DupBase:
    pass


try:

    class DupSub(DupBase, DupBase):
        pass

    assert False, 'expected duplicate base TypeError'
except TypeError as e:
    assert str(e) == 'duplicate base class DupBase'

try:

    class DupObjSub(object, object):
        pass

    assert False, 'expected duplicate object base TypeError'
except TypeError as e:
    assert str(e) == 'duplicate base class object'


# Test MRO conflict error message with base class names
class MroX:
    pass


class MroY(MroX):
    pass


try:

    class MroConflict(MroX, MroY):
        pass

    assert False, 'expected MRO conflict TypeError'
except TypeError as e:
    assert str(e) == 'Cannot create a consistent method resolution order (MRO) for bases MroX, MroY'

# === Special "dunder" method inheritance ===


class ContainerBase:
    def __init__(self, data):
        self.data = data

    def __str__(self):
        return 'Container(' + str(self.data) + ')'

    def __eq__(self, other):
        if type(self) is type(other):
            return self.data == other.data
        return False

    def __contains__(self, item):
        return item in self.data

    def __iter__(self):
        return iter(self.data)


class MyContainer(ContainerBase):
    def is_empty(self):
        return len(self.data) == 0


c1 = MyContainer([1, 2, 3])
c2 = MyContainer([1, 2, 3])
c3 = MyContainer([4, 5])

assert str(c1) == 'Container([1, 2, 3])'
assert c1 == c2
assert not (c1 == c3)
assert 2 in c1
assert 9 not in c1

items = []
for x in c3:
    items.append(x)
assert items == [4, 5]
assert not c1.is_empty()


# Test object ordering in bases
class ObjBase:
    pass


class ValidObjSub(ObjBase, object):
    pass


assert isinstance(ValidObjSub(), ObjBase)

try:

    class InvalidObjSub(object, ObjBase):
        pass

    assert False, 'expected MRO conflict with object first'
except TypeError as e:
    assert str(e) == 'Cannot create a consistent method resolution order (MRO) for bases object, ObjBase'


class ObjA:
    pass


class ObjB:
    pass


try:

    class InvalidMidObjSub(ObjA, object, ObjB):
        pass

    assert False, 'expected MRO conflict with object in middle'
except TypeError as e:
    assert str(e) == 'Cannot create a consistent method resolution order (MRO) for bases object, ObjB'


# Test class decorators with inheritance
def add_custom_attr(cls):
    cls.custom_attr = 'decorated'
    return cls


@add_custom_attr
class DecoratedSub(Parent):
    pass


assert DecoratedSub.custom_attr == 'decorated'
assert DecoratedSub.y == 2


# Test dynamic base expression evaluated at runtime
def choose_base():
    return Grandparent


class DynamicSub(choose_base()):
    pass


assert DynamicSub.x == 1


# Test negative isinstance checks
class UnrelatedClass:
    pass


assert not isinstance(c, UnrelatedClass)
assert not isinstance(123, Parent)
assert not isinstance([1, 2], Parent)


# Test user-defined class named 'object' is not treated as builtin object
def test_user_class_named_object():
    class object:
        custom = True

    class CustomBase:
        x = 1

    class SubCustomObj(object, CustomBase):
        pass

    assert SubCustomObj.custom is True
    assert SubCustomObj.x == 1


test_user_class_named_object()


# Test nested class inheriting from enclosing sibling in function body
def test_nested_classes():
    class SiblingBase:
        flag = 'from_sibling'

    class SiblingChild(SiblingBase):
        pass

    assert SiblingChild.flag == 'from_sibling'
    assert isinstance(SiblingChild(), SiblingBase)


test_nested_classes()


# Test frozen dataclass subclass computes field-wise hash
from dataclasses import dataclass


@dataclass(frozen=True)
class FrozenBase:
    pass


@dataclass(frozen=True)
class FrozenChild(FrozenBase):
    y: int


fc1 = FrozenChild(1)
fc2 = FrozenChild(1)
fc3 = FrozenChild(2)
assert hash(fc1) == hash(fc2)
assert hash(fc1) != hash(fc3)


# Test starred unpacking in class bases
class StarBaseA:
    val_a = 'a'


class StarBaseB:
    val_b = 'b'


star_bases = (StarBaseA, StarBaseB)


class StarChild(*star_bases):  # type: ignore[reportUntypedBaseClass]
    pass


assert StarChild.val_a == 'a'
assert StarChild.val_b == 'b'
assert isinstance(StarChild(), StarBaseA)
assert isinstance(StarChild(), StarBaseB)

# --- class__inheritance_basic.py ---
# Single inheritance: inherited initialization, methods, and attribute lookup.


class Parent:
    label = 'parent'

    def __init__(self, value, offset=1):
        self.value = value + offset

    def read(self):
        return self.value

    def describe(self):
        return self.label, self.read()


class Child(Parent):
    label = 'child'

    def read(self):
        return self.value * 2


class Grandchild(Child):
    pass


parent = Parent(10)
child = Child(10, offset=2)
grandchild = Grandchild(10)
assert parent.describe() == ('parent', 11)
assert child.describe() == ('child', 24)
assert grandchild.describe() == ('child', 22)
assert Parent.read(child) == 12
assert Child.read(child) == 24
assert Grandchild.read(grandchild) == 22
assert type(grandchild) is Grandchild

bound = grandchild.describe
assert bound() == ('child', 22)
grandchild.label = 'instance'
assert bound() == ('instance', 22)
assert Grandchild.label == 'child'


class OwnInit(Parent):
    def __init__(self):
        self.value = 7


assert OwnInit().read() == 7

try:
    child.missing
    assert False, 'missing attributes must raise AttributeError'
except AttributeError:
    pass

# --- class__inheritance_lookup.py ---
# Class and instance lookup follow the parent chain; instance methods bind the child.
# pyright: reportRedeclaration=false


class Parent:
    label = 'parent'

    def read(self):
        return self.value

    def describe(self):
        return self.label, self.read()


class Child(Parent):
    label = 'child'

    def read(self):
        return self.value * 2


class Grandchild(Child):
    pass


instance = Grandchild()
instance.value = 7
assert instance.describe() == ('child', 14)
assert Parent.read(instance) == 7
assert Child.read(instance) == Grandchild.read(instance) == 14
assert Child.describe(instance) == Grandchild.describe(instance) == ('child', 14)
assert getattr(instance, 'describe')() == ('child', 14)
assert getattr(Grandchild, 'read')(instance) == 14
assert hasattr(instance, 'describe')
assert hasattr(Grandchild, 'describe')
assert not hasattr(instance, 'missing')
assert not hasattr(Grandchild, 'missing')
assert getattr(instance, 'missing', 42) == 42
assert getattr(Grandchild, 'missing', 42) == 42

instance.label = 'instance'
assert instance.describe() == ('instance', 14)
assert Grandchild.label == 'child'
assert Grandchild.__name__ == 'Grandchild'
assert type(instance) is Grandchild

for obj in [instance, Grandchild]:
    try:
        obj.missing
    except AttributeError:
        pass
    else:
        assert False, 'lookup must fail after checking all ancestors'

Dynamic = type('Dynamic', (Grandchild,), {})
dynamic = Dynamic()
dynamic.value = 3
assert dynamic.describe() == ('child', 6)
assert Dynamic.describe(dynamic) == ('child', 6)

OriginalParent = Parent


class Parent:
    label = 'replacement'


assert Child.label == 'child'
OriginalParent.extra = 'original'
Parent.extra = 'replacement'
assert instance.extra == Grandchild.extra == 'original'

# --- class__inheritance_mutation.py ---
# Parent changes remain visible through descendants unless shadowed.


class Parent:
    value = 1
    items = []

    def read(self):
        return self.value


class Child(Parent):
    pass


class Grandchild(Child):
    pass


child = Child()
grandchild = Grandchild()
saved_method = child.read
Parent.value = 2
assert Child.value == Grandchild.value == 2
assert child.read() == grandchild.read() == 2

Child.value = 3
assert Parent.value == 2
assert child.value == grandchild.value == 3
child.value = 4
assert child.value == 4
assert Child.value == grandchild.value == 3

Child.items.append('shared')
assert Parent.items is Child.items
assert Grandchild.items == ['shared']
Child.items = ['child']
assert Parent.items == ['shared']
assert Grandchild.items is Child.items


def replacement(self):
    return self.value * 10


Parent.read = replacement
assert child.read() == 40
assert grandchild.read() == 30
assert saved_method() == 4, 'an already bound method keeps its original function'


def added(self):
    return self.value + 100


Parent.added = added
assert grandchild.added() == 103
Child.added = None
assert grandchild.added is None, 'None shadows a parent member'

# --- class__inheritance_isinstance.py ---
# isinstance follows ancestors, including tuple and union classinfo.


class Parent:
    pass


class Child(Parent):
    pass


class Grandchild(Child):
    pass


class Other:
    pass


instance = Grandchild()
assert type(instance) is Grandchild
assert isinstance(instance, Grandchild)
assert isinstance(instance, Child)
assert isinstance(instance, Parent)
assert not isinstance(instance, Other)
assert not isinstance(Parent(), Child)
assert not isinstance(Child, Parent)
assert not isinstance(42, Parent)
assert not isinstance([], Parent)
assert isinstance(instance, (Other, Parent))
assert isinstance(instance, (Other, (int, Parent)))
assert not isinstance(instance, (Other, int))
assert not isinstance(instance, ())
assert isinstance(instance, Other | Parent)
assert not isinstance(instance, Other | int)

assert isinstance(instance, (Parent, 42)), 'a matching ancestor short-circuits invalid later entries'
try:
    isinstance(instance, (42, Parent))
except TypeError:
    pass
else:
    assert False, 'invalid earlier entries must raise TypeError'

# --- class__inheritance_dynamic.py ---
# type() accepts one user-defined parent and copies the supplied namespace.


class Parent:
    def __init__(self, value):
        self.value = value

    def read(self):
        return self.value


namespace = {'label': 'dynamic'}
Child = type('Child', (Parent,), namespace)
Grandchild = type('Grandchild', (Child,), {})
instance = Grandchild(42)
assert Child.__name__ == 'Child'
assert instance.label == 'dynamic'
assert instance.read() == 42
assert Parent.read(instance) == 42
assert isinstance(instance, Parent)
assert isinstance(instance, Child)
assert type(instance) is Grandchild
namespace['label'] = 'changed'
assert Child.label == 'dynamic'

Parent.extra = 10
assert Grandchild.extra == instance.extra == 10

try:
    type('Invalid', (42,), {})
    assert False, 'a non-class parent must raise TypeError'
except TypeError:
    pass

# --- class__inheritance_redefinition.py ---
# Redefinition rebinds the name; descendants retain the original parent object.
# pyright: reportRedeclaration=false


class Parent:
    value = 'original'


OriginalParent = Parent


class Child(Parent):
    pass


OriginalChild = Child
old_child = Child()


class Parent:
    value = 'replacement'


assert old_child.value == Child.value == 'original'
assert isinstance(old_child, OriginalParent)
assert not isinstance(old_child, Parent)
OriginalParent.value = 'changed'
assert old_child.value == Child.value == 'changed'


class Child(Parent):
    pass


new_child = Child()
assert new_child.value == 'replacement'
assert type(old_child) is OriginalChild
assert type(new_child) is Child
assert not isinstance(old_child, Child)
assert isinstance(new_child, Parent)
assert not isinstance(new_child, OriginalParent)

# --- class__inheritance_dunders.py ---
# Special methods inherit and reflect later changes to the parent.


class Parent:
    def __init__(self, value):
        self.value = value

    def __repr__(self):
        return f'Value({self.value})'

    def __eq__(self, other):
        if not isinstance(other, Parent):
            return NotImplemented
        return self.value == other.value

    def __hash__(self):
        return self.value

    def __contains__(self, item):
        return item == self.value

    def __enter__(self):
        return self.value

    def __exit__(self, exc_type, exc, traceback):
        self.closed = True
        return False


class Child(Parent):
    pass


instance = Child(7)
assert repr(instance) == 'Value(7)'
assert str(instance) == 'Value(7)'
assert instance == Parent(7)
assert instance != Child(8)
assert hash(instance) == 7
assert {instance: 'found'}[Child(7)] == 'found'
assert 7 in instance
assert 8 not in instance
with instance as value:
    assert value == 7
assert instance.closed


def new_repr(self):
    return 'changed'


Parent.__repr__ = new_repr
assert repr(instance) == str(instance) == 'changed'


class Unhashable(Parent):
    def __eq__(self, other):
        return True


try:
    hash(Unhashable(7))
    assert False, 'defining __eq__ disables an inherited __hash__'
except TypeError:
    pass

assert Unhashable.__hash__ is None


class Grandchild(Child):
    pass


grandchild = Grandchild(9)
assert repr(grandchild) == 'changed'
assert hash(grandchild) == 9


def new_hash(self):
    return 23


Parent.__hash__ = new_hash
assert hash(instance) == hash(grandchild) == 23
# A method on the instance does not replace an implicit special-method lookup.
instance.__repr__ = lambda: 'instance'
assert instance.__repr__() == 'instance'
assert repr(instance) == 'changed'


class Iterator:
    def __init__(self):
        self.current = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.current == 3:
            raise StopIteration
        self.current += 1
        return self.current


class ChildIterator(Iterator):
    pass


assert list(ChildIterator()) == [1, 2, 3]
assert next(ChildIterator()) == 1
assert 2 in ChildIterator()


class NoIteration(ChildIterator):
    __iter__ = None


try:
    iter(NoIteration())
except TypeError:
    pass
else:
    assert False, 'None must shadow an inherited iterator method'


class NoContains(ChildIterator):
    __contains__ = None


try:
    2 in NoContains()
except TypeError:
    pass
else:
    assert False, 'None must disable containment without falling back to iteration'


class Index:
    def __index__(self):
        return 2


class ChildIndex(Index):
    pass


assert [10, 20, 30][ChildIndex()] == 30


class Text:
    def __str__(self):
        return 'parent text'


class ChildText(Text):
    pass


assert str(ChildText()) == 'parent text'


class Overrides(Parent):
    def __repr__(self):
        return 'child'


assert repr(Overrides(1)) == 'child'
ExplicitHash = type('ExplicitHash', (Parent,), {'__eq__': lambda self, other: True, '__hash__': Parent.__hash__})
assert hash(ExplicitHash(1)) == 23

# --- class__inheritance_exceptions.py ---
# Custom exceptions retain their identity when raised, caught, and re-raised.


class AppError(Exception):
    pass


class InputError(AppError):
    pass


error = InputError('invalid input')
assert error.args == ('invalid input',)
assert str(error) == 'invalid input'
assert isinstance(error, InputError)
assert isinstance(error, AppError)
assert isinstance(error, Exception)

try:
    raise error
except InputError as caught:
    assert caught is error
else:
    assert False, 'the custom class must catch its own instances'

try:
    raise error
except AppError as caught:
    assert caught is error
else:
    assert False, 'the parent class must catch a descendant'

try:
    try:
        raise error
    except AppError:
        raise
except Exception as caught:
    assert caught is error
else:
    assert False, 'bare raise must preserve custom exception identity'

try:
    raise AppError
except AppError as caught:
    assert type(caught) is AppError
    assert caught.args == ()
else:
    assert False, 'raising a custom class must construct an instance'

assert repr(AppError()) == 'AppError()'
assert str(AppError()) == ''
assert str(AppError(42)) == '42'
assert repr(AppError(42)) == 'AppError(42)'
assert AppError(1, 2, 3).args == (1, 2, 3)
assert str(AppError(1, 2, 3)) == '(1, 2, 3)'
assert repr(AppError(1, 2, 3)) == 'AppError(1, 2, 3)'

DynamicError = type('DynamicError', (AppError,), {})
assert isinstance(DynamicError('dynamic'), Exception)
try:
    raise DynamicError('dynamic')
except AppError as caught:
    assert type(caught) is DynamicError


class MissingKey(KeyError):
    pass


assert str(MissingKey(42)) == '42'
assert str(MissingKey('key')) == "'key'"


class OtherError(Exception):
    pass


def fail():
    raise error


try:
    fail()
except OtherError:
    assert False, 'siblings must not match'
except (OtherError, AppError) as caught:
    assert caught is error
else:
    assert False, 'function unwinding must preserve custom exceptions'

try:
    try:
        raise error
    except (AppError, 42):
        assert False, 'the whole handler tuple must be validated'
except TypeError:
    pass
else:
    assert False


class Stop(BaseException):
    pass


assert isinstance(Stop(), BaseException)
assert not isinstance(Stop(), Exception)
try:
    raise Stop('stop')
except Exception:
    assert False
except Stop as caught:
    assert str(caught) == 'stop'


class DetailedError(AppError):
    def __init__(self, message, *, code=7):
        self.code = code


class ChildError(DetailedError):
    pass


detailed = ChildError('details', code=9)
assert detailed.code == 9
assert detailed.args == ('details',)
try:
    raise detailed
except DetailedError as caught:
    assert caught is detailed
    assert caught.code == 9


class Manager:
    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, traceback):
        assert exc_type is InputError
        assert exc is error
        return True


with Manager():
    raise error


class Plain:
    pass


for invalid in [Plain, Plain()]:
    try:
        raise invalid
    except TypeError:
        pass
    else:
        assert False, 'plain classes are not exceptions'

# --- class__inheritance_type_checks.py ---
# Instance and subclass checks follow the complete parent chain.

from collections import namedtuple


class Parent:
    pass


class Child(Parent):
    pass


class Grandchild(Child):
    pass


class Other:
    pass


instance = Grandchild()
assert type(instance) is Grandchild
assert instance.__class__ is Grandchild
assert isinstance(instance, Grandchild)
assert isinstance(instance, Child)
assert isinstance(instance, Parent)
assert isinstance(instance, (Other, Parent))
assert not isinstance(instance, Other)
assert not isinstance(Parent(), Child)
assert not isinstance(Child, Parent)

assert issubclass(Grandchild, Grandchild)
assert issubclass(Grandchild, Child)
assert issubclass(Grandchild, Parent)
assert issubclass(Grandchild, (Other, Parent))
assert not issubclass(Parent, Child)
assert not issubclass(Grandchild, Other)

assert issubclass(Grandchild, object)
assert issubclass(Grandchild, (Other, (Child,)))
assert issubclass(Grandchild, Other | Parent)
assert not issubclass(Parent, Child | Other)
assert not issubclass(Grandchild, ())
assert not issubclass(42, ())
assert issubclass(Grandchild, (Parent, 42))

assert issubclass(bool, int)
assert not issubclass(int, bool)
assert issubclass(int, object)
assert issubclass(type, type)
assert issubclass(type(None), object)
assert issubclass(ValueError, Exception)
assert issubclass(KeyError, LookupError)
assert not issubclass(BaseException, Exception)
assert issubclass(Exception, object)


assert not issubclass(Child, Exception)
assert not isinstance(Child(), Exception)

Point = namedtuple('Point', ['x'])
OtherPoint = namedtuple('Point', ['x'])
assert issubclass(Point, Point)
assert issubclass(Point, tuple)
assert issubclass(Point, object)
assert not issubclass(Point, OtherPoint)


def check_error(call, expected):
    try:
        call()
    except TypeError as error:
        assert str(error) == expected, str(error)
    else:
        assert False, 'expected TypeError'


for invalid in [42, Grandchild(), list[int], int | str]:
    check_error(lambda: issubclass(invalid, Parent), 'issubclass() arg 1 must be a class')

check_error(lambda: issubclass(42, 42), 'issubclass() arg 1 must be a class')
check_error(lambda: issubclass(Grandchild, 42), 'issubclass() arg 2 must be a class, a tuple of classes, or a union')
check_error(
    lambda: issubclass(Grandchild, (Other, 42)),
    'issubclass() arg 2 must be a class, a tuple of classes, or a union',
)
check_error(lambda: issubclass(int, list[int]), 'issubclass() argument 2 cannot be a parameterized generic')
check_error(lambda: issubclass(int, str | list[int]), 'issubclass() argument 2 cannot be a parameterized generic')
assert issubclass(int, int | list[int])
check_error(lambda: issubclass(), 'issubclass expected 2 arguments, got 0')
check_error(lambda: issubclass(int), 'issubclass expected 2 arguments, got 1')
check_error(lambda: issubclass(int, str, object), 'issubclass expected 2 arguments, got 3')
check_error(lambda: issubclass(int, classinfo=int), 'issubclass() takes no keyword arguments')

print('success')
