# === type() as three-argument class constructor ===
MyClass = type('MyClass', (), {'x': 42, 'greet': lambda self: 'hello'})
obj = MyClass()
assert obj.x == 42, 'type() constructor class attr'
assert obj.greet() == 'hello', 'type() constructor method'
assert type(obj) is MyClass, 'type() constructor instance type'
assert MyClass.__name__ == 'MyClass', 'type() constructor name'

# === type() with bases ===
Base = type('Base', (), {'base_attr': 'from base'})
Child = type('Child', (Base,), {'child_attr': 'from child'})
c = Child()
assert c.base_attr == 'from base', 'type() with base class'
assert c.child_attr == 'from child', 'type() child attr'
assert isinstance(c, Base), 'type() child isinstance base'


# === __set_name__ on descriptors during class creation ===
class DescriptorWithName:
    def __set_name__(self, owner, name):
        self.public_name = name
        self.private_name = '_' + name

    def __get__(self, obj, objtype=None):
        if obj is None:
            return self
        return getattr(obj, self.private_name, None)

    def __set__(self, obj, value):
        setattr(obj, self.private_name, value)


class Model:
    name = DescriptorWithName()
    age = DescriptorWithName()

    def __init__(self, name, age):
        self.name = name
        self.age = age


m = Model('Alice', 30)
assert m.name == 'Alice', '__set_name__ descriptor get'
assert m.age == 30, '__set_name__ descriptor get 2'

name_desc = Model.__dict__['name']
assert name_desc.public_name == 'name', '__set_name__ public_name'
assert name_desc.private_name == '_name', '__set_name__ private_name'


# === __class_getitem__ for generic classes ===
class Stack:
    def __init__(self):
        self.items = []

    def push(self, item):
        self.items.append(item)

    def pop(self):
        return self.items.pop()

    def __class_getitem__(cls, item):
        return cls  # simplified: just return the class


int_stack = Stack[int]
assert int_stack is Stack, '__class_getitem__ returns cls'


# === __class_getitem__ with custom return ===
class TypedContainer:
    def __class_getitem__(cls, params):
        return (cls, params)


result = TypedContainer[int]
assert result == (TypedContainer, int), '__class_getitem__ custom return'


# === __init_subclass__ is not called for the defining class itself ===
class TrackingBase:
    subclass_count = 0

    def __init_subclass__(cls):
        TrackingBase.subclass_count += 1


assert TrackingBase.subclass_count == 0, '__init_subclass__ not called for defining class'


class Sub1(TrackingBase):
    pass


assert TrackingBase.subclass_count == 1, '__init_subclass__ called for Sub1'


class Sub2(TrackingBase):
    pass


assert TrackingBase.subclass_count == 2, '__init_subclass__ called for Sub2'

# === type() on single arg returns type ===
assert type(42) is int, 'type(42) is int'
assert type('hello') is str, 'type("hello") is str'
assert type([]) is list, 'type([]) is list'
assert type(True) is bool, 'type(True) is bool'
