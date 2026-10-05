"""Every class hierarchy ends at object; __bases__ lists direct parents."""


class Parent:
    pass


class Child(Parent):
    pass


class Items(list):
    pass


assert object.__bases__ == ()
assert Parent.__bases__ == (object,)
assert Child.__bases__ == (Parent,)
assert Items.__bases__ == (list,)
assert list.__bases__ == (object,)
assert bool.__bases__ == (int,)
assert int.__bases__ == (object,)
assert ValueError.__bases__ == (Exception,)
assert Exception.__bases__ == (BaseException,)
assert BaseException.__bases__ == (object,)
assert type.__bases__ == (object,)

for cls in (Parent, Child, Items, list, bool, int, ValueError, Exception, type):
    assert issubclass(cls, object)
    assert cls.__class__ is type
    print(cls.__name__, 'has direct bases', cls.__bases__)

for value in (Child(), Items([1]), True, 42, 'hello', ValueError('example')):
    assert isinstance(value, object)
