# === Single inheritance with method override ===
class Animal:
    def __init__(self, name):
        self.name = name

    def speak(self):
        return self.name + ' makes a sound'


class Dog(Animal):
    def speak(self):
        return self.name + ' barks'


class Cat(Animal):
    def speak(self):
        return self.name + ' meows'


d = Dog('Rex')
assert d.name == 'Rex', 'inherited attribute from parent'
assert d.speak() == 'Rex barks', 'overridden method'

c = Cat('Whiskers')
assert c.name == 'Whiskers', 'inherited attribute from parent (cat)'
assert c.speak() == 'Whiskers meows', 'overridden method (cat)'

a = Animal('Generic')
assert a.speak() == 'Generic makes a sound', 'base class method'


# === Inherited method not overridden ===
class Vehicle:
    def __init__(self, speed):
        self.speed = speed

    def describe(self):
        return 'speed=' + str(self.speed)


class Car(Vehicle):
    def __init__(self, speed, brand):
        self.speed = speed
        self.brand = brand


car = Car(100, 'Toyota')
assert car.describe() == 'speed=100', 'inherited method not overridden'
assert car.brand == 'Toyota', 'child-specific attribute'


# === super().__init__() call chain ===
class Base:
    def __init__(self, x):
        self.x = x


class Child(Base):
    def __init__(self, x, y):
        super().__init__(x)
        self.y = y


class GrandChild(Child):
    def __init__(self, x, y, z):
        super().__init__(x, y)
        self.z = z


gc = GrandChild(1, 2, 3)
assert gc.x == 1, 'super chain: grandchild x from base'
assert gc.y == 2, 'super chain: grandchild y from child'
assert gc.z == 3, 'super chain: grandchild z'


# === super().method() delegation ===
class Logger:
    def log(self):
        return 'Logger'


class FileLogger(Logger):
    def log(self):
        return super().log() + '->FileLogger'


class RotatingFileLogger(FileLogger):
    def log(self):
        return super().log() + '->RotatingFileLogger'


rfl = RotatingFileLogger()
assert rfl.log() == 'Logger->FileLogger->RotatingFileLogger', 'super().method() chain'

# === isinstance with user classes ===
assert isinstance(d, Dog), 'isinstance direct class'
assert isinstance(d, Animal), 'isinstance parent class'
assert not isinstance(d, Cat), 'not isinstance unrelated child'
assert isinstance(c, Animal), 'isinstance cat is Animal'
assert not isinstance(c, Dog), 'not isinstance cat is not Dog'
assert isinstance(gc, GrandChild), 'isinstance direct'
assert isinstance(gc, Child), 'isinstance grandparent'
assert isinstance(gc, Base), 'isinstance great-grandparent'

# === issubclass checks ===
assert issubclass(Dog, Animal), 'issubclass Dog Animal'
assert issubclass(Cat, Animal), 'issubclass Cat Animal'
assert issubclass(Dog, Dog), 'issubclass self'
assert issubclass(Animal, Animal), 'issubclass self base'
assert not issubclass(Animal, Dog), 'not issubclass parent of child'
assert not issubclass(Dog, Cat), 'not issubclass siblings'
assert issubclass(GrandChild, Base), 'issubclass transitive'
assert issubclass(GrandChild, Child), 'issubclass grandchild child'

# === isinstance with tuple of types ===
assert isinstance(d, (Dog, Cat)), 'isinstance tuple match first'
assert isinstance(c, (Dog, Cat)), 'isinstance tuple match second'
assert not isinstance(a, (Dog, Cat)), 'isinstance tuple no match'
assert isinstance(d, (int, str, Dog)), 'isinstance tuple mixed types'

# === issubclass with tuple of types ===
assert issubclass(Dog, (Animal, int)), 'issubclass tuple'
assert not issubclass(Dog, (Cat, int)), 'issubclass tuple no match'


# === Multiple inheritance ===
class A:
    def method(self):
        return 'A'


class B(A):
    def method(self):
        return 'B'


class C(A):
    def method(self):
        return 'C'


class D(B, C):
    pass


d_obj = D()
assert d_obj.method() == 'B', 'MRO: D -> B -> C -> A, B wins'

# === MRO order verification ===
assert D.__mro__ == (D, B, C, A, object), 'D MRO tuple'
assert B.__mro__ == (B, A, object), 'B MRO tuple'
assert C.__mro__ == (C, A, object), 'C MRO tuple'


# === Diamond inheritance ===
class DiamondBase:
    def __init__(self):
        self.log = []
        self.log.append('DiamondBase')


class Left(DiamondBase):
    def __init__(self):
        super().__init__()
        self.log.append('Left')


class Right(DiamondBase):
    def __init__(self):
        super().__init__()
        self.log.append('Right')


class Bottom(Left, Right):
    def __init__(self):
        super().__init__()
        self.log.append('Bottom')


b = Bottom()
# MRO: Bottom -> Left -> Right -> DiamondBase -> object
# super().__init__() chains: Bottom->Left->Right->DiamondBase
assert b.log == ['DiamondBase', 'Right', 'Left', 'Bottom'], 'diamond init order'

# === Diamond MRO verification ===
assert Bottom.__mro__ == (Bottom, Left, Right, DiamondBase, object), 'diamond MRO'


# === Multiple inheritance attribute resolution ===
class M1:
    x = 1


class M2:
    x = 2
    y = 20


class M3(M1, M2):
    pass


m = M3()
assert m.x == 1, 'MRO: M1.x wins over M2.x'
assert m.y == 20, 'M2.y found via MRO'

# === isinstance with multiple inheritance ===
assert isinstance(d_obj, D), 'isinstance D'
assert isinstance(d_obj, B), 'isinstance B via MI'
assert isinstance(d_obj, C), 'isinstance C via MI'
assert isinstance(d_obj, A), 'isinstance A via MI'
assert isinstance(d_obj, object), 'isinstance object'

# === issubclass with multiple inheritance ===
assert issubclass(D, B), 'issubclass D B'
assert issubclass(D, C), 'issubclass D C'
assert issubclass(D, A), 'issubclass D A'
assert issubclass(D, object), 'issubclass D object'


# === super() in multiple inheritance follows MRO ===
class MBase:
    def method(self):
        return ['MBase']


class MLeft(MBase):
    def method(self):
        return super().method() + ['MLeft']


class MRight(MBase):
    def method(self):
        return super().method() + ['MRight']


class MDiamond(MLeft, MRight):
    def method(self):
        return super().method() + ['MDiamond']


md = MDiamond()
assert md.method() == ['MBase', 'MRight', 'MLeft', 'MDiamond'], 'super follows MRO in diamond'


# === Inheriting __init__ from parent ===
class ParentInit:
    def __init__(self, val):
        self.val = val


class ChildNoInit(ParentInit):
    def double(self):
        return self.val * 2


cn = ChildNoInit(5)
assert cn.val == 5, 'inherited __init__'
assert cn.double() == 10, 'child method uses inherited attr'


# === Class with object as explicit base ===
class ExplicitObject(object):
    pass


eo = ExplicitObject()
assert isinstance(eo, object), 'explicit object base'
assert type(eo) is ExplicitObject, 'type is ExplicitObject'


# === Three-level deep single inheritance ===
class Level1:
    def who(self):
        return 'Level1'


class Level2(Level1):
    pass


class Level3(Level2):
    pass


l3 = Level3()
assert l3.who() == 'Level1', 'method found 2 levels up'
assert isinstance(l3, Level1), 'isinstance 2 levels up'
assert issubclass(Level3, Level1), 'issubclass 2 levels up'


# === Method override at middle level ===
class Top:
    def greet(self):
        return 'Top'


class Middle(Top):
    def greet(self):
        return 'Middle'


class Bottom2(Middle):
    pass


b2 = Bottom2()
assert b2.greet() == 'Middle', 'method from middle level wins'
