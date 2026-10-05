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
