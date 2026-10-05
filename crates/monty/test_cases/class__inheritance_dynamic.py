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
