import base64
import binascii
import random


class Sized:
    def __init__(self, result):
        self.result = result

    def __len__(self):
        return self.result


class Truth:
    def __init__(self, result):
        self.result = result

    def __bool__(self):
        return self.result


class Index:
    def __init__(self, result):
        self.result = result

    def __index__(self):
        return self.result


def check_error(operation, value, error_type, message):
    """Check protocol failures without discarding their precise error messages."""
    try:
        operation(value)
        assert False, 'expected protocol operation to raise'
    except error_type as exc:
        assert str(exc) == message


# === Length validation and explicit calls ===
for raw, expected in [(0, 0), (3, 3), (False, 0), (True, 1), (2**63 - 1, 2**63 - 1), (Index(4), 4)]:
    obj = Sized(raw)
    assert len(obj) == expected
    assert type(len(obj)) is int
    assert bool(obj) is (expected != 0)
    assert obj.__len__() is raw

for operation in [len, bool]:
    for raw in [-1, -(2**63), -(10**100), Index(-1), Index(-(10**100))]:
        check_error(operation, Sized(raw), ValueError, '__len__() should return >= 0')
    for raw in [2**63, 10**100, Index(10**100)]:
        check_error(operation, Sized(raw), OverflowError, "cannot fit 'int' into an index-sized integer")
    for raw, name in [('bad', 'str'), (None, 'NoneType'), (NotImplemented, 'NotImplementedType'), ([], 'list')]:
        check_error(operation, Sized(raw), TypeError, f"'{name}' object cannot be interpreted as an integer")
    check_error(operation, Sized(Index('bad')), TypeError, '__index__ returned non-int (type str)')

assert Sized(-1).__len__() == -1
assert Sized(10**100).__len__() == 10**100
assert Sized('raw').__len__() == 'raw'

# === Boolean validation and precedence ===
assert bool(Truth(True)) is True
assert bool(Truth(False)) is False
assert Truth('raw').__bool__() == 'raw'
for raw, name in [
    (1, 'int'),
    (None, 'NoneType'),
    (NotImplemented, 'NotImplementedType'),
    ([], 'list'),
    (Sized(0), 'Sized'),
]:
    check_error(bool, Truth(raw), TypeError, f'__bool__ should return bool, returned {name}')


class Both:
    def __bool__(self):
        return False

    def __len__(self):
        raise ValueError('length called')


assert bool(Both()) is False
check_error(len, Both(), ValueError, 'length called')


class BadBool:
    def __bool__(self):
        return 1

    def __len__(self):
        raise ValueError('must not fall back')


check_error(bool, BadBool(), TypeError, '__bool__ should return bool, returned int')


# === Missing hooks, noncallable hooks, and class-only lookup ===
class Plain:
    pass


plain = Plain()
plain.__len__ = lambda: 0
plain.__bool__ = lambda: False
assert bool(plain) is True
check_error(len, plain, TypeError, "object of type 'Plain' has no len()")
assert plain.__len__() == 0
assert plain.__bool__() is False

sized = Sized(2)
sized.__len__ = lambda: 0
sized.__bool__ = lambda: False
assert len(sized) == 2
assert bool(sized) is True


class NoLength:
    __len__ = None


class NoBool:
    __bool__ = None

    def __len__(self):
        return 0


for operation in [len, bool]:
    check_error(operation, NoLength(), TypeError, "'NoneType' object is not callable")
check_error(bool, NoBool(), TypeError, "'NoBool' cannot be interpreted as a boolean")
assert len(NoBool()) == 0

Plain.__len__ = lambda self: 0
assert len(plain) == 0
assert bool(plain) is False
Plain.__bool__ = lambda self: True
assert bool(plain) is True
assert len(plain) == 0

# === Conditions and native consumers ===
empty = Sized(0)
full = Sized(2)
assert (not empty) is True
assert (not full) is False
assert (empty or full) is full, 'or must test the left operand'
assert (full and empty) is empty, 'and must test the left operand'
assert ('yes' if full else 'no') == 'yes'
assert ('yes' if empty else 'no') == 'no'
assert all([full, empty]) is False
assert any([empty, full]) is True
assert list(filter(None, [empty, full])) == [full]
assert [i for i in [0, 1, 2] if Sized(i)] == [1, 2]


class Countdown:
    def __init__(self):
        self.remaining = 3

    def __bool__(self):
        self.remaining -= 1
        return self.remaining >= 0


counter = Countdown()
visits = []
while counter:
    visits.append(counter.remaining)
assert visits == [2, 1, 0]


class Container:
    def __init__(self, result):
        self.result = result

    def __contains__(self, item):
        return self.result


assert (1 in Container(empty)) is False
assert (1 in Container(full)) is True
check_error(lambda obj: 1 in obj, Container(Truth(0)), TypeError, '__bool__ should return bool, returned int')


class Equal:
    def __eq__(self, other):
        return Sized(0)


assert bool(Equal() == 1) is False
assert (Equal() in [1]) is False


class Manager:
    def __enter__(self):
        return self

    def __exit__(self, typ, value, tb):
        return Sized(1)


with Manager():
    raise ValueError('suppressed by length truthiness')


# === Exceptions leave callers and temporary return values intact ===
class Raising:
    def __len__(self):
        raise ValueError('length failed')

    def __bool__(self):
        raise ValueError('truth failed')


class RaisingIndex:
    def __index__(self):
        raise ValueError('index failed')


check_error(len, Raising(), ValueError, 'length failed')
check_error(bool, Raising(), ValueError, 'truth failed')
check_error(len, Sized(RaisingIndex()), ValueError, 'index failed')
check_error(lambda obj: 1 if obj else 0, Raising(), ValueError, 'truth failed')
check_error(lambda obj: any([obj]), Raising(), ValueError, 'truth failed')
check_error(lambda obj: all([obj]), Raising(), ValueError, 'truth failed')
check_error(lambda obj: list(filter(None, [obj])), Raising(), ValueError, 'truth failed')


class FreshIndex:
    def __len__(self):
        return Index(2)


class FreshInvalid:
    def __len__(self):
        return ['length']

    def __bool__(self):
        return ['truth']


for _ in range(10):
    assert len(FreshIndex()) == 2
    check_error(len, FreshInvalid(), TypeError, "'list' object cannot be interpreted as an integer")
    check_error(bool, FreshInvalid(), TypeError, '__bool__ should return bool, returned list')


# === Length can mutate its receiver and re-enter other length operations ===
class Mutating:
    def __init__(self):
        self.items = [10, 20]

    def __len__(self):
        self.items = [30]
        return len(Sized(len(self.items)))

    def __getitem__(self, index):
        return self.items[index]


assert random.Random(42).choice(Mutating()) == 30
check_error(lambda obj: binascii.hexlify(b'a', obj), Sized(2), ValueError, 'sep must be length 1.')
check_error(lambda obj: binascii.hexlify(b'a', obj), Sized(1), TypeError, 'sep must be str or bytes.')
check_error(lambda obj: base64.b64encode(b'a', obj), Raising(), ValueError, 'length failed')

# === Range lengths use the Python index width, not allocation size ===
assert len(range(2**40)) == 2**40
assert len(range(2**63 - 1)) == 2**63 - 1
huge_range = range(-(2**63), 2**63 - 1)
assert bool(huge_range) is True
check_error(len, huge_range, OverflowError, 'Python int too large to convert to C ssize_t')

try:
    match huge_range:
        case []:
            assert False, 'expected pattern length to overflow'
        case _:
            assert False, 'expected pattern length to overflow'
except OverflowError as exc:
    assert str(exc) == 'Python int too large to convert to C ssize_t'
