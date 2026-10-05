# Special methods inherit and reflect later changes to the parent.


class Parent:
    def __init__(self, value):
        self.value = value

    def __repr__(self):
        return f'Value({self.value})'

    def __eq__(self, other):
        if not isinstance(other, Parent):
            return NotImplemented
        return self.value == other.value

    def __hash__(self):
        return self.value

    def __contains__(self, item):
        return item == self.value

    def __enter__(self):
        return self.value

    def __exit__(self, exc_type, exc, traceback):
        self.closed = True
        return False


class Child(Parent):
    pass


instance = Child(7)
assert repr(instance) == 'Value(7)'
assert str(instance) == 'Value(7)'
assert instance == Parent(7)
assert instance != Child(8)
assert hash(instance) == 7
assert {instance: 'found'}[Child(7)] == 'found'
assert 7 in instance
assert 8 not in instance
with instance as value:
    assert value == 7
assert instance.closed


def new_repr(self):
    return 'changed'


Parent.__repr__ = new_repr
assert repr(instance) == str(instance) == 'changed'


class Unhashable(Parent):
    def __eq__(self, other):
        return True


try:
    hash(Unhashable(7))
    assert False, 'defining __eq__ disables an inherited __hash__'
except TypeError:
    pass

assert Unhashable.__hash__ is None


class Grandchild(Child):
    pass


grandchild = Grandchild(9)
assert repr(grandchild) == 'changed'
assert hash(grandchild) == 9


def new_hash(self):
    return 23


Parent.__hash__ = new_hash
assert hash(instance) == hash(grandchild) == 23
# A method on the instance does not replace an implicit special-method lookup.
instance.__repr__ = lambda: 'instance'
assert instance.__repr__() == 'instance'
assert repr(instance) == 'changed'


class Iterator:
    def __init__(self):
        self.current = 0

    def __iter__(self):
        return self

    def __next__(self):
        if self.current == 3:
            raise StopIteration
        self.current += 1
        return self.current


class ChildIterator(Iterator):
    pass


assert list(ChildIterator()) == [1, 2, 3]
assert next(ChildIterator()) == 1
assert 2 in ChildIterator()


class NoIteration(ChildIterator):
    __iter__ = None


try:
    iter(NoIteration())
except TypeError:
    pass
else:
    assert False, 'None must shadow an inherited iterator method'


class NoContains(ChildIterator):
    __contains__ = None


try:
    2 in NoContains()
except TypeError:
    pass
else:
    assert False, 'None must disable containment without falling back to iteration'


class Index:
    def __index__(self):
        return 2


class ChildIndex(Index):
    pass


assert [10, 20, 30][ChildIndex()] == 30


class Text:
    def __str__(self):
        return 'parent text'


class ChildText(Text):
    pass


assert str(ChildText()) == 'parent text'


class Overrides(Parent):
    def __repr__(self):
        return 'child'


assert repr(Overrides(1)) == 'child'
ExplicitHash = type('ExplicitHash', (Parent,), {'__eq__': lambda self, other: True, '__hash__': Parent.__hash__})
assert hash(ExplicitHash(1)) == 23
