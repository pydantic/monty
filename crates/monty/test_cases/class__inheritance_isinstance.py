# isinstance follows ancestors, including tuple and union classinfo.


class Parent:
    pass


class Child(Parent):
    pass


class Grandchild(Child):
    pass


class Other:
    pass


instance = Grandchild()
assert type(instance) is Grandchild
assert isinstance(instance, Grandchild)
assert isinstance(instance, Child)
assert isinstance(instance, Parent)
assert not isinstance(instance, Other)
assert not isinstance(Parent(), Child)
assert not isinstance(Child, Parent)
assert not isinstance(42, Parent)
assert not isinstance([], Parent)
assert isinstance(instance, (Other, Parent))
assert isinstance(instance, (Other, (int, Parent)))
assert not isinstance(instance, (Other, int))
assert not isinstance(instance, ())
assert isinstance(instance, Other | Parent)
assert not isinstance(instance, Other | int)

assert isinstance(instance, (Parent, 42)), 'a matching ancestor short-circuits invalid later entries'
try:
    isinstance(instance, (42, Parent))
except TypeError:
    pass
else:
    assert False, 'invalid earlier entries must raise TypeError'
