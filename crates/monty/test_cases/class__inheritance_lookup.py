# Class and instance lookup follow the parent chain; instance methods bind the child.


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
