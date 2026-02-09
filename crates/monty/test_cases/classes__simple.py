# === Basic class definition and instantiation ===
class Dog:
    species = 'Canis familiaris'

    def __init__(self, name, age):
        self.name = name
        self.age = age

    def bark(self):
        return self.name + ' says woof'


d = Dog('Rex', 5)
assert d.name == 'Rex', 'instance attr name'
assert d.age == 5, 'instance attr age'
assert d.bark() == 'Rex says woof', 'method call'
assert Dog.species == 'Canis familiaris', 'class attr via class'
assert d.species == 'Canis familiaris', 'class attr via instance'


# === Class with no __init__ ===
class Empty:
    pass


e = Empty()

# === type() returns the class ===
assert type(d) is Dog, 'type() returns class'
assert type(e) is Empty, 'type() returns class for empty'

# === isinstance() with user-defined class ===
assert isinstance(d, Dog), 'isinstance with own class'
assert not isinstance(d, Empty), 'isinstance with other class'
assert isinstance(e, Empty), 'isinstance with own class'

# === isinstance with tuple ===
assert isinstance(d, (Dog, Empty)), 'isinstance with tuple of classes'
assert isinstance(d, (int, Dog)), 'isinstance with mixed tuple'
assert not isinstance(d, (int, str)), 'isinstance no match'

# === Multiple instances ===
d2 = Dog('Buddy', 3)
assert d2.name == 'Buddy', 'second instance has own attrs'
assert d.name == 'Rex', 'first instance unchanged'


# === Class variable cross-references ===
class VarRef:
    a = 1
    b = a + 2
    c = b * 3


assert VarRef.a == 1, 'first class var'
assert VarRef.b == 3, 'second class var references first'
assert VarRef.c == 9, 'third class var references second'

# === Class body executes at definition time ===
trace = []


class Tracer:
    trace.append('class body')

    def __init__(self):
        trace.append('init')


assert trace == ['class body'], 'class body runs at definition'
Tracer()
assert trace == ['class body', 'init'], 'init runs at instantiation'
Tracer()
assert trace == ['class body', 'init', 'init'], 'init runs each time'


# === No __init__ with args raises TypeError ===
class NoInit:
    pass


NoInit()  # OK, no args

try:
    NoInit(1, 2, 3)
    assert False, 'should raise TypeError'
except TypeError as e:
    assert 'takes no arguments' in str(e), 'error message format'


# === Nested class ===
class Outer:
    outer_var = 'outer'

    class Inner:
        inner_var = 'inner'


assert Outer.Inner.inner_var == 'inner', 'nested class accessible via outer'
assert Outer.outer_var == 'outer', 'outer var still accessible'


# === Instance with keyword arguments ===
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y


p = Point(x=10, y=20)
assert p.x == 10, 'kwarg x'
assert p.y == 20, 'kwarg y'

p2 = Point(1, y=2)
assert p2.x == 1, 'positional + kwarg'
assert p2.y == 2, 'kwarg y'


# === Method with return value ===
class Calculator:
    def __init__(self):
        self.total = 0

    def add(self, n):
        self.total = self.total + n
        return self


c = Calculator()
c.add(10)
c.add(20)
assert c.total == 30, 'mutable state via methods'


# === Method referencing class attribute ===
class Counter:
    count = 0

    def __init__(self):
        Counter.count = Counter.count + 1

    def get_count(self):
        return Counter.count


c1 = Counter()
assert c1.get_count() == 1, 'first counter'
c2 = Counter()
assert c2.get_count() == 2, 'second counter'
assert c1.get_count() == 2, 'first counter sees update'
