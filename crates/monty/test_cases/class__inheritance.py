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

    class SubCustomObj(object, ObjA):
        pass

    assert SubCustomObj.custom is True
    assert SubCustomObj.x == 1 if hasattr(SubCustomObj, 'x') else True


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


print('success')
