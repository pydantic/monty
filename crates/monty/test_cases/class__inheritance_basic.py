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
