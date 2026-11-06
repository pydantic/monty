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


class StarChild(*star_bases):
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

# === Stage 1.3: inherited __init_subclass__, __mro__ write guard, dynamic __annotations__, frozen dataclass ===
hook_log = []


class HookRoot:
    def __init_subclass__(cls, **kwargs):
        super().__init_subclass__()
        hook_log.append((cls.__name__, kwargs))


class HookMid(HookRoot):
    pass


hook_log.clear()
HookMid.__init_subclass__(flag=1)
fn_hook = HookMid.__init_subclass__
fn_hook(flag=2)
hm_inst = HookMid()
hm_inst.__init_subclass__(flag=3)
fn_inst_hook = hm_inst.__init_subclass__
fn_inst_hook(flag=4)
assert hook_log == [
    ('HookMid', {'flag': 1}),
    ('HookMid', {'flag': 2}),
    ('HookMid', {'flag': 3}),
    ('HookMid', {'flag': 4}),
]

try:
    HookMid.__mro__ = ()
    assert False, 'expected AttributeError writing __mro__'
except AttributeError as exc:
    assert str(exc) == "attribute '__mro__' of 'type' objects is not writable"

DynChild = type('DynChild', (HookRoot,), {})
ann1 = DynChild.__annotations__
ann2 = DynChild.__annotations__
assert ann1 == {}
assert ann1 is ann2
ann1['added'] = 'int'
assert DynChild.__annotations__ == {'added': 'int'}

from dataclasses import dataclass


@dataclass(frozen=True)
class FrozenDC:
    x: int


try:

    @dataclass
    class MutableChildDC(FrozenDC):
        y: int

    assert False, 'expected TypeError inheriting non-frozen from frozen'
except TypeError as exc:
    assert str(exc) == 'cannot inherit non-frozen dataclass from a frozen one'


@dataclass
class MutableDC:
    x: int


try:

    @dataclass(frozen=True)
    class FrozenChildDC(MutableDC):
        y: int

    assert False, 'expected TypeError inheriting frozen from non-frozen'
except TypeError as exc:
    assert str(exc) == 'cannot inherit frozen dataclass from a non-frozen one'


# --- Subclass reflected binary operator and rich equality precedence ---
class OpBase:
    def __add__(self, o):
        return 'base.add'

    def __sub__(self, o):
        return 'base.sub'

    def __mul__(self, o):
        return 'base.mul'

    def __matmul__(self, o):
        return 'base.matmul'

    def __truediv__(self, o):
        return 'base.truediv'

    def __floordiv__(self, o):
        return 'base.floordiv'

    def __mod__(self, o):
        return 'base.mod'

    def __divmod__(self, o):
        return 'base.divmod'

    def __pow__(self, o, m=None):
        return ('base.pow', m)

    def __and__(self, o):
        return 'base.and'

    def __or__(self, o):
        return 'base.or'

    def __xor__(self, o):
        return 'base.xor'

    def __lshift__(self, o):
        return 'base.lshift'

    def __rshift__(self, o):
        return 'base.rshift'


class OpSub(OpBase):
    def __radd__(self, o):
        return 'sub.radd'

    def __rsub__(self, o):
        return 'sub.rsub'

    def __rmul__(self, o):
        return 'sub.rmul'

    def __rmatmul__(self, o):
        return 'sub.rmatmul'

    def __rtruediv__(self, o):
        return 'sub.rtruediv'

    def __rfloordiv__(self, o):
        return 'sub.rfloordiv'

    def __rmod__(self, o):
        return 'sub.rmod'

    def __rdivmod__(self, o):
        return 'sub.rdivmod'

    def __rpow__(self, o, m=None):
        return ('sub.rpow', m)

    def __rand__(self, o):
        return 'sub.rand'

    def __ror__(self, o):
        return 'sub.ror'

    def __rxor__(self, o):
        return 'sub.rxor'

    def __rlshift__(self, o):
        return 'sub.rlshift'

    def __rrshift__(self, o):
        return 'sub.rrshift'


class OpInherited(OpBase):
    pass


ob = OpBase()
os_inst = OpSub()
oi = OpInherited()
assert ob + os_inst == 'sub.radd'
assert ob - os_inst == 'sub.rsub'
assert ob * os_inst == 'sub.rmul'
assert ob @ os_inst == 'sub.rmatmul'
assert ob / os_inst == 'sub.rtruediv'
assert ob // os_inst == 'sub.rfloordiv'
assert ob % os_inst == 'sub.rmod'
assert divmod(ob, os_inst) == 'sub.rdivmod'
assert ob**os_inst == ('sub.rpow', None)
assert pow(ob, os_inst, 7) == ('sub.rpow', 7)
assert (ob & os_inst) == 'sub.rand'
assert (ob | os_inst) == 'sub.ror'
assert (ob ^ os_inst) == 'sub.rxor'
assert (ob << os_inst) == 'sub.rlshift'
assert (ob >> os_inst) == 'sub.rrshift'

# Same-class and unoverridden subclass fall back to Base direct dunder first.
assert os_inst + os_inst == 'base.add'
assert ob + oi == 'base.add'

# Inherited or identical __radd__ does NOT preempt Base.__add__, unless Base.__add__ declines.
radd_log = []


class RaddBase:
    def __add__(self, o):
        radd_log.append(('add', type(self).__name__))
        return 'base.add'

    def __radd__(self, o):
        radd_log.append(('radd', type(self).__name__))
        return 'base.radd'


class RaddSubInherited(RaddBase):
    pass


assert RaddBase() + RaddSubInherited() == 'base.add'
assert radd_log == [('add', 'RaddBase')]

radd_decline_log = []


class RaddDeclineBase:
    def __add__(self, o):
        radd_decline_log.append(('add', type(self).__name__))
        return NotImplemented

    def __radd__(self, o):
        radd_decline_log.append(('radd', type(self).__name__))
        return 'fallback.radd'


class RaddDeclineSub(RaddDeclineBase):
    pass


assert RaddDeclineBase() + RaddDeclineSub() == 'fallback.radd'
assert radd_decline_log == [('add', 'RaddDeclineBase'), ('radd', 'RaddDeclineSub')]

# Sub overriding __radd__ and returning NotImplemented falls back to Base.__add__ once.
sub_decline_log = []


class SubDecline(OpBase):
    def __radd__(self, o):
        sub_decline_log.append('sub.radd')
        return NotImplemented


assert OpBase() + SubDecline() == 'base.add'
assert sub_decline_log == ['sub.radd']


# Shared function object on both classes is not considered overloaded.
def shared_radd(self, o):
    return 'shared.radd'


class SharedBase:
    def __add__(self, o):
        return 'shared.add'

    __radd__ = shared_radd


class SharedSub(SharedBase):
    __radd__ = shared_radd


assert SharedBase() + SharedSub() == 'shared.add'


# Overriding __radd__ with None preempts Base.__add__ and raises TypeError.
class NoneRaddSub(RaddBase):
    __radd__ = None


try:
    RaddBase() + NoneRaddSub()
    assert False, 'expected TypeError calling None __radd__'
except TypeError as exc:
    assert str(exc) == "'NoneType' object is not callable"

# Rich equality (== and !=) tries strict subclass first even when inherited.
eq_log = []


class EqBase:
    def __eq__(self, o):
        eq_log.append((type(self).__name__, type(o).__name__))
        return True


class EqSubInherited(EqBase):
    pass


class EqSubOverride(EqBase):
    def __eq__(self, o):
        eq_log.append(('override', type(self).__name__, type(o).__name__))
        return False


assert EqBase() == EqSubInherited()
assert eq_log == [('EqSubInherited', 'EqBase')]
eq_log.clear()
assert not (EqBase() == EqSubOverride())
assert EqBase() != EqSubOverride()
assert eq_log == [
    ('override', 'EqSubOverride', 'EqBase'),
    ('override', 'EqSubOverride', 'EqBase'),
]


# object.__setattr__ stores args on a plain instance as given
plain_inst_args = EqBase()
object.__setattr__(plain_inst_args, 'args', [10, 20])
assert plain_inst_args.args == [10, 20]


# Foo.__annotations__() on an annotated class raises TypeError, not panic
class AnnFoo:
    x: int


try:
    AnnFoo.__annotations__([1, 2])
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == "'dict' object is not callable"


# instance_attr and py_call_attr agreement on __annotations__ and custom __class__
class FreshAnn:
    a: int


inst_fresh = FreshAnn()
try:
    _ = inst_fresh.__annotations__
    assert False, 'expected AttributeError'
except AttributeError:
    pass

try:
    inst_fresh.__annotations__()
    assert False, 'expected AttributeError'
except AttributeError:
    pass


class CustomClassAttr:
    __class__ = lambda self: 'custom'


cca = CustomClassAttr()
assert cca.__class__() == 'custom'
assert (cca.__class__)() == 'custom'
assert type(cca) is CustomClassAttr

# object.__init__ on immutable built-ins ignores excess args, while types overriding tp_init raise
assert object.__init__(1, 2) is None
assert object.__init__('hello', k=1) is None
assert object.__init__((1, 2), 'extra') is None

for _builtin_inst in [ValueError('orig'), [1, 2], {'a': 1}, {1, 2}]:
    try:
        object.__init__(_builtin_inst, 1)
        assert False, 'expected TypeError'
    except TypeError as exc:
        assert str(exc) == 'object.__init__() takes exactly one argument (the instance to initialize)'


# super().__setattr__ fallback
class SuperSetAttr(object):
    def set_val(self, k, v):
        super().__setattr__(k, v)


ssa = SuperSetAttr()
ssa.set_val('my_attr', 42)
assert ssa.my_attr == 42

print('success')
