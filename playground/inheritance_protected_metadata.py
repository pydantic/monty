"""Monty's WIP rule: class identity and direct bases cannot be reassigned.

Run this example with Monty. CPython permits some of these assignments.
"""


class Parent:
    pass


class Child(Parent):
    pass


instance = Child()
for target in (Parent, Child, instance):
    for name in ('__class__', '__bases__'):
        try:
            setattr(target, name, Parent)
        except TypeError:
            print('Rejected assignment to', name)
        else:
            raise AssertionError('Class metadata must be protected')

# The native setter must enforce the same rule.
try:
    object.__setattr__(instance, '__class__', Parent)
except TypeError:
    print('object.__setattr__ also protects __class__')
else:
    raise AssertionError('Native setter changed the class')

assert instance.__class__ is Child
assert Child.__bases__ == (Parent,)
