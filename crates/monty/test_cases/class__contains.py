# A user class implementing `__contains__` decides `in` / `not in` itself.
#
# It takes precedence over `__iter__`, so a class defining both is never
# iterated by `in`; a class defining neither is a TypeError.


# === __contains__ alone: no __iter__ needed ===
class Membership:
    def __init__(self, allowed):
        self.allowed = allowed

    def __contains__(self, item):
        return item in self.allowed


m = Membership(['a', 'b'])
assert 'a' in m
assert 'z' not in m


# === __contains__ takes precedence over __iter__ ===
# The iteration answer and the __contains__ answer are deliberately opposite,
# so this fails loudly if the fallback is reached.
class Both:
    def __iter__(self):
        return iter([1, 2, 3])

    def __contains__(self, item):
        return item == 99


b = Both()
assert 99 in b
assert 1 not in b
# iteration itself is unaffected — only `in` consults __contains__
assert list(b) == [1, 2, 3]
assert [x for x in b] == [1, 2, 3]


# === the result is coerced by truthiness, not required to be a bool ===
class Truthy:
    def __contains__(self, item):
        return item


assert 5 in Truthy()
assert 0 not in Truthy()
assert [0] in Truthy()
assert [] not in Truthy()


class NoneContains:
    def __contains__(self, item):
        return None


assert 1 not in NoneContains()
assert isinstance(1 in NoneContains(), bool)


class EmptyContainer:
    def __contains__(self, item):
        return []


assert 1 not in EmptyContainer()


# === __contains__ = None opts out of `in` entirely ===
# The None is never called, and — unlike an absent __contains__ — there is no
# fallback to iteration, so a class defining __iter__ too still raises.
class OptOut:
    __contains__ = None


try:
    1 in OptOut()
    assert False, 'expected TypeError for an opted-out __contains__'
except TypeError as e:
    assert str(e) == "'OptOut' object is not a container"


class OptOutIterable:
    def __iter__(self):
        return iter([1, 2])

    __contains__ = None


try:
    1 in OptOutIterable()
    assert False, 'expected TypeError rather than a fallback to iteration'
except TypeError as e:
    assert str(e) == "'OptOutIterable' object is not a container"
try:
    1 not in OptOutIterable()
    assert False, 'expected TypeError from `not in` as well'
except TypeError as e:
    assert str(e) == "'OptOutIterable' object is not a container"
# iteration itself is unaffected — only `in` is opted out
assert list(OptOutIterable()) == [1, 2]


# === __contains__ receives the item, and self when it is a plain function ===
class Recorder:
    def __init__(self):
        self.seen = []

    def __contains__(self, item):
        self.seen.append(item)
        return False


r = Recorder()
assert 'x' not in r
assert 'y' not in r
assert r.seen == ['x', 'y']


# === without __contains__, `in` still falls back to iteration ===
class IterOnly:
    def __iter__(self):
        return iter(['p', 'q'])


assert 'p' in IterOnly()
assert 'z' not in IterOnly()


# === exceptions from __contains__ propagate unchanged ===
class Boom:
    def __contains__(self, item):
        raise ValueError('nope')


try:
    1 in Boom()
    assert False, 'expected ValueError from __contains__'
except ValueError as e:
    assert str(e) == 'nope'
try:
    1 not in Boom()
    assert False, 'expected ValueError from __contains__ via not in'
except ValueError as e:
    assert str(e) == 'nope'


# === a non-callable __contains__ is a TypeError when `in` invokes it ===
class NotCallable:
    __contains__ = 42


try:
    1 in NotCallable()
    assert False, 'expected TypeError for non-callable __contains__'
except TypeError as e:
    assert str(e) == "'int' object is not callable"


# === __contains__ is looked up on the class, never the instance __dict__ ===
class InstanceOnly:
    def __init__(self):
        self.__contains__ = lambda item: True


try:
    1 in InstanceOnly()
    assert False, 'expected TypeError for instance-only __contains__'
except TypeError as e:
    assert str(e) == "argument of type 'InstanceOnly' is not a container or iterable"


# === __contains__ may call back into `in` on itself ===
class Nested:
    def __init__(self, inner):
        self.inner = inner

    def __contains__(self, item):
        return item in self.inner


assert 1 in Nested(Nested([1, 2]))
assert 3 not in Nested(Nested([1, 2]))
