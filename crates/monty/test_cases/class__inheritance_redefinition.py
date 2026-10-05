# Redefinition rebinds the name; descendants retain the original parent object.


class Parent:
    value = 'original'


OriginalParent = Parent


class Child(Parent):
    pass


OriginalChild = Child
old_child = Child()


class Parent:
    value = 'replacement'


assert old_child.value == Child.value == 'original'
assert isinstance(old_child, OriginalParent)
assert not isinstance(old_child, Parent)
OriginalParent.value = 'changed'
assert old_child.value == Child.value == 'changed'


class Child(Parent):
    pass


new_child = Child()
assert new_child.value == 'replacement'
assert type(old_child) is OriginalChild
assert type(new_child) is Child
assert not isinstance(old_child, Child)
assert isinstance(new_child, Parent)
assert not isinstance(new_child, OriginalParent)
