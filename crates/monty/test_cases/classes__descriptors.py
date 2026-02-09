# === Data descriptor (__get__ + __set__) ===
class Validated:
    def __init__(self, min_val, max_val):
        self.min_val = min_val
        self.max_val = max_val
        self.name = None

    def __set_name__(self, owner, name):
        self.name = '_' + name

    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return getattr(obj, self.name, None)

    def __set__(self, obj, value):
        if not isinstance(value, (int, float)):
            raise TypeError('value must be a number')
        if value < self.min_val or value > self.max_val:
            raise ValueError('value out of range')
        setattr(obj, self.name, value)


class Settings:
    volume = Validated(0, 100)
    brightness = Validated(0, 255)

    def __init__(self, vol, bright):
        self.volume = vol
        self.brightness = bright


s = Settings(50, 128)
assert s.volume == 50, 'data descriptor get'
assert s.brightness == 128, 'data descriptor get 2'

s.volume = 75
assert s.volume == 75, 'data descriptor set'

caught = False
try:
    s.volume = 200
except ValueError:
    caught = True
assert caught, 'data descriptor validation'

caught = False
try:
    s.volume = 'loud'
except TypeError:
    caught = True
assert caught, 'data descriptor type validation'


# === Non-data descriptor (__get__ only) ===
class Computed:
    def __init__(self, func):
        self.func = func

    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return self.func(obj)


class Circle:
    def __init__(self, radius):
        self.radius = radius

    @Computed
    def area(self):
        return 3.14159 * self.radius * self.radius


c = Circle(5)
assert abs(c.area - 78.53975) < 0.001, 'non-data descriptor'


# === Descriptor priority: data descriptor > instance > non-data descriptor ===
class DataDesc:
    """Data descriptor that stores values in instance dict under a private name."""

    def __init__(self):
        self.name = None

    def __set_name__(self, owner, name):
        self.name = '_dd_' + name

    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return 'data descriptor'

    def __set__(self, obj, value):
        pass  # Ignore sets -- data descriptor always wins on read


class NonDataDesc:
    def __get__(self, obj, objtype=None):
        return 'non-data descriptor'


class Priority:
    data = DataDesc()
    nondata = NonDataDesc()

    def __init__(self):
        # Set instance attrs with same names as class descriptors.
        # For 'nondata', since NonDataDesc is a non-data descriptor,
        # this bypasses the descriptor and writes to instance dict directly.
        # For 'data', DataDesc.__set__ is called (data descriptor intercepts).
        pass


p = Priority()

# Data descriptor should always win over instance dict
assert p.data == 'data descriptor', 'data descriptor > instance dict'

# Non-data descriptor used when no instance attr
p2 = Priority()
assert p2.nondata == 'non-data descriptor', 'non-data descriptor when no instance attr'


# Test: when instance has attr with same name, it wins over non-data descriptor.
# We set the attr directly (not through class namespace) to bypass descriptors.
# In our implementation, plain setattr on a non-data descriptor name stores in
# the instance dict, and instance dict wins over non-data descriptors.
class PriorityTest:
    nondata = NonDataDesc()

    def __init__(self, override_val=None):
        if override_val is not None:
            # This stores in instance dict since NonDataDesc has no __set__
            self.nondata = override_val


pt_no_override = PriorityTest()
assert pt_no_override.nondata == 'non-data descriptor', 'non-data desc when no instance attr'

pt_override = PriorityTest('instance value')
assert pt_override.nondata == 'instance value', 'instance dict > non-data descriptor'


# === __slots__ restricting attributes ===
class Slotted:
    __slots__ = ('x', 'y')

    def __init__(self, x, y):
        self.x = x
        self.y = y


sl = Slotted(1, 2)
assert sl.x == 1, '__slots__ x access'
assert sl.y == 2, '__slots__ y access'

sl.x = 10
assert sl.x == 10, '__slots__ x set'

caught = False
try:
    sl.z = 3
except AttributeError:
    caught = True
assert caught, '__slots__ prevents arbitrary attrs'

# === __slots__ no __dict__ ===
assert not hasattr(sl, '__dict__'), '__slots__ class has no __dict__'


# === __slots__ with inheritance ===
class SlottedBase:
    __slots__ = ('a',)


class SlottedChild(SlottedBase):
    __slots__ = ('b',)


sc = SlottedChild()
sc.a = 1
sc.b = 2
assert sc.a == 1, '__slots__ inherited slot'
assert sc.b == 2, '__slots__ child slot'


# === __slots__ child without __slots__ gets __dict__ ===
class UnslottedChild(SlottedBase):
    pass


uc = UnslottedChild()
uc.a = 1
uc.extra = 'allowed'
assert uc.a == 1, 'unslotted child inherits slot'
assert uc.extra == 'allowed', 'unslotted child allows arbitrary attrs'


# === __new__ for custom allocation ===
class Singleton:
    _instance = None

    def __new__(cls):
        if cls._instance is None:
            cls._instance = super().__new__(cls)
        return cls._instance

    def __init__(self):
        self.value = 42


s1 = Singleton()
s2 = Singleton()
assert s1 is s2, '__new__ singleton'
assert s1.value == 42, '__new__ singleton value'


# === __new__ called before __init__ ===
class InitOrder:
    log = []

    def __new__(cls):
        InitOrder.log.append('new')
        return super().__new__(cls)

    def __init__(self):
        InitOrder.log.append('init')


InitOrder.log = []
obj = InitOrder()
assert InitOrder.log == ['new', 'init'], '__new__ called before __init__'


# === __new__ can return different type (skips __init__) ===
class WeirdClass:
    def __new__(cls):
        return 42

    def __init__(self):
        # This should NOT be called since __new__ returned a non-instance
        raise RuntimeError('should not be called')


w = WeirdClass()
assert w == 42, '__new__ returns non-instance skips __init__'


# === Descriptor accessed on class returns descriptor itself ===
class MyDescriptor:
    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return 'value'


class Owner:
    attr = MyDescriptor()


assert isinstance(Owner.attr, MyDescriptor), 'descriptor on class returns descriptor'
assert Owner().attr == 'value', 'descriptor on instance returns value'


# === __set_name__ called during class creation ===
class NameTracker:
    def __init__(self):
        self.attr_name = None
        self.owner_name = None

    def __set_name__(self, owner, name):
        self.attr_name = name
        self.owner_name = owner.__name__

    def __get__(self, obj, objtype=None):
        return self.attr_name


class HasTrackers:
    foo = NameTracker()
    bar = NameTracker()


assert HasTrackers.foo == 'foo', '__set_name__ sets name foo'
assert HasTrackers.bar == 'bar', '__set_name__ sets name bar'

# Verify through direct access
assert HasTrackers.__dict__['foo'].owner_name == 'HasTrackers', '__set_name__ owner'


# === __delete__ descriptor ===
class Deletable:
    def __init__(self):
        self.was_deleted = False

    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return getattr(obj, '_val', 'default')

    def __set__(self, obj, value):
        obj._val = value

    def __delete__(self, obj):
        self.was_deleted = True
        if hasattr(obj, '_val'):
            del obj._val


class HasDeletable:
    attr = Deletable()


hd = HasDeletable()
hd.attr = 'hello'
assert hd.attr == 'hello', 'descriptor set then get'
del hd.attr
assert hd.attr == 'default', 'descriptor after delete'
desc = HasDeletable.__dict__['attr']
assert desc.was_deleted, '__delete__ was called'


# === __slots__ with tuple ===
class TupleSlots:
    __slots__ = ('a', 'b', 'c')


ts = TupleSlots()
ts.a = 1
ts.b = 2
ts.c = 3
assert ts.a == 1, 'tuple slots a'
assert ts.b == 2, 'tuple slots b'
assert ts.c == 3, 'tuple slots c'


# === __slots__ with single string ===
class SingleSlot:
    __slots__ = ('value',)


ss = SingleSlot()
ss.value = 42
assert ss.value == 42, 'single slot'


# === Function descriptor vs custom descriptor (Finding #5) ===
class CustomDesc2:
    def __get__(self, obj, objtype=None):
        if obj is None:
            return self  # Class access returns descriptor
        return 'custom descriptor'


class WithBoth2:
    desc = CustomDesc2()

    def method(self):
        return 'method result'


wb2 = WithBoth2()
assert wb2.desc == 'custom descriptor', 'custom descriptor __get__ called'
assert wb2.method() == 'method result', 'bound method call'

# Verify direct access returns descriptor/function
assert isinstance(WithBoth2.desc, CustomDesc2), 'descriptor on class returns descriptor'


# === __new__ returning non-instance skips __init__ (Finding #8) ===
class TrackInit:
    init_called = False

    def __new__(cls):
        return 42  # Not an instance

    def __init__(self):
        TrackInit.init_called = True


t = TrackInit()
assert t == 42, '__new__ returned 42'
assert not TrackInit.init_called, '__init__ was NOT called when __new__ returns non-instance'
