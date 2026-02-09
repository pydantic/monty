# === @staticmethod basic ===
class MathUtils:
    @staticmethod
    def add(a, b):
        return a + b

    @staticmethod
    def multiply(a, b):
        return a * b


assert MathUtils.add(2, 3) == 5, 'staticmethod called on class'
assert MathUtils.multiply(4, 5) == 20, 'staticmethod called on class 2'

m = MathUtils()
assert m.add(2, 3) == 5, 'staticmethod called on instance'
assert m.multiply(4, 5) == 20, 'staticmethod called on instance 2'


# === @staticmethod no self parameter ===
class NoSelf:
    @staticmethod
    def greet(name):
        return 'Hello, ' + name


assert NoSelf.greet('World') == 'Hello, World', 'staticmethod no self on class'
assert NoSelf().greet('World') == 'Hello, World', 'staticmethod no self on instance'


# === @classmethod basic ===
class Counter:
    count = 0

    def __init__(self):
        Counter.count += 1

    @classmethod
    def get_count(cls):
        return cls.count

    @classmethod
    def reset(cls):
        cls.count = 0


assert Counter.get_count() == 0, 'classmethod before any instances'
Counter()
Counter()
assert Counter.get_count() == 2, 'classmethod after instances'
Counter.reset()
assert Counter.get_count() == 0, 'classmethod reset'


# === @classmethod receives cls ===
class Animal:
    def __init__(self, name):
        self.name = name

    @classmethod
    def create(cls, name):
        return cls(name)


a = Animal.create('Rex')
assert a.name == 'Rex', 'classmethod factory'
assert type(a) is Animal, 'classmethod factory type'

# === @classmethod on instance ===
c = Counter()
assert c.get_count() == 1, 'classmethod called on instance'


# === @classmethod with inheritance ===
class Base:
    kind = 'base'

    @classmethod
    def get_kind(cls):
        return cls.kind


class Child(Base):
    kind = 'child'


assert Base.get_kind() == 'base', 'classmethod on base'
assert Child.get_kind() == 'child', 'classmethod on child uses child cls'


# === @classmethod as factory with inheritance ===
class Shape:
    def __init__(self, name):
        self.name = name

    @classmethod
    def create(cls, name):
        return cls(name)


class Circle(Shape):
    pass


s = Shape.create('shape1')
c = Circle.create('circle1')
assert type(s) is Shape, 'factory returns Shape'
assert type(c) is Circle, 'factory returns Circle via inheritance'
assert c.name == 'circle1', 'factory sets name on child'


# === @property getter ===
class Temperature:
    def __init__(self, celsius):
        self._celsius = celsius

    @property
    def celsius(self):
        return self._celsius

    @property
    def fahrenheit(self):
        return self._celsius * 9 / 5 + 32


t = Temperature(100)
assert t.celsius == 100, 'property getter'
assert t.fahrenheit == 212.0, 'computed property'


# === @property setter ===
class Person:
    def __init__(self, name):
        self._name = name

    @property
    def name(self):
        return self._name

    @name.setter
    def name(self, value):
        if not isinstance(value, str):
            raise TypeError('name must be a string')
        self._name = value


p = Person('Alice')
assert p.name == 'Alice', 'property getter'
p.name = 'Bob'
assert p.name == 'Bob', 'property setter'

caught = False
try:
    p.name = 123
except TypeError as e:
    caught = True
    assert str(e) == 'name must be a string', 'property setter validation'
assert caught, 'property setter raised TypeError'


# === @property deleter ===
class OptionalField:
    def __init__(self, value):
        self._value = value

    @property
    def value(self):
        return self._value

    @value.setter
    def value(self, v):
        self._value = v

    @value.deleter
    def value(self):
        self._value = None


of = OptionalField(42)
assert of.value == 42, 'property before delete'
del of.value
assert of.value is None, 'property after delete'


# === @property with inheritance ===
class BaseRect:
    def __init__(self, w, h):
        self._w = w
        self._h = h

    @property
    def area(self):
        return self._w * self._h


class Square(BaseRect):
    def __init__(self, side):
        super().__init__(side, side)


sq = Square(5)
assert sq.area == 25, 'inherited property'


# === @property override in child ===
class BaseClass:
    @property
    def info(self):
        return 'base'


class ChildClass(BaseClass):
    @property
    def info(self):
        return 'child'


assert BaseClass().info == 'base', 'base property'
assert ChildClass().info == 'child', 'overridden property'


# === @property is not callable like a function ===
class PropTest:
    @property
    def val(self):
        return 42


pt = PropTest()
assert pt.val == 42, 'property accessed as attribute not function call'


# === Combining @staticmethod, @classmethod, @property ===
class Combined:
    _instance_count = 0

    def __init__(self, value):
        self._value = value
        Combined._instance_count += 1

    @property
    def value(self):
        return self._value

    @value.setter
    def value(self, v):
        self._value = v

    @staticmethod
    def helper(x):
        return x * 2

    @classmethod
    def count(cls):
        return cls._instance_count

    @classmethod
    def create(cls, v):
        return cls(v)


c1 = Combined.create(10)
c2 = Combined.create(20)
assert c1.value == 10, 'combined: property getter'
c1.value = 15
assert c1.value == 15, 'combined: property setter'
assert Combined.helper(3) == 6, 'combined: staticmethod on class'
assert c1.helper(3) == 6, 'combined: staticmethod on instance'
assert Combined.count() == 2, 'combined: classmethod'


# === @staticmethod and @classmethod can access class attributes ===
class Config:
    debug = False

    @staticmethod
    def is_debug():
        return Config.debug

    @classmethod
    def set_debug(cls, val):
        cls.debug = val


assert Config.is_debug() == False, 'staticmethod reads class attr'
Config.set_debug(True)
assert Config.is_debug() == True, 'classmethod modifies class attr'
Config.set_debug(False)


# === Read-only property (no setter) raises AttributeError ===
class ReadOnly:
    @property
    def x(self):
        return 42


ro = ReadOnly()
assert ro.x == 42, 'read-only property getter'
caught = False
try:
    ro.x = 99
except AttributeError:
    caught = True
assert caught, 'read-only property raises AttributeError on set'


# === @staticmethod with default args ===
class Defaults:
    @staticmethod
    def greet(name, greeting='Hello'):
        return greeting + ', ' + name


assert Defaults.greet('World') == 'Hello, World', 'staticmethod default arg'
assert Defaults.greet('World', 'Hi') == 'Hi, World', 'staticmethod override default'


# === @classmethod with default args ===
class Factory:
    @classmethod
    def make(cls, x, y=0):
        return (x, y)


assert Factory.make(1) == (1, 0), 'classmethod default arg'
assert Factory.make(1, 2) == (1, 2), 'classmethod override default'


# === Multiple decorators execute bottom-up (Finding #15) ===
def tracker1(cls):
    cls.log.append('tracker1')
    return cls


def tracker2(cls):
    cls.log.append('tracker2')
    return cls


def tracker3(cls):
    cls.log.append('tracker3')
    return cls


@tracker1
@tracker2
@tracker3
class DecoOrder:
    log = []


# Execution order: tracker3 → tracker2 → tracker1 (bottom to top)
assert DecoOrder.log == ['tracker3', 'tracker2', 'tracker1'], 'decorators bottom-up'
