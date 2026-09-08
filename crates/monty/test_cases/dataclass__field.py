from dataclasses import MISSING, dataclass, field

# === default_factory builds a fresh value per instance ===


@dataclass
class Bag:
    xs: list[int] = field(default_factory=list)


a, b = Bag(), Bag()
assert a.xs == []
assert a.xs is not b.xs
a.xs.append(1)
assert a.xs == [1]
assert b.xs == []

# An explicit argument wins over the factory, which is then never called.
assert Bag([9]).xs == [9]


# === field(default=...) is the plain default ===
@dataclass
class Plain:
    a: int = field(default=5)
    b: int = 7


assert Plain() == Plain(5, 7)
assert repr(Plain()) == 'Plain(a=5, b=7)'
assert Plain(1, 2).a == 1

# === The spec never survives as a class attribute ===
# A plain default becomes the class attribute, as an undecorated `b: int = 7`
# already is; a factory has no single value to expose, so it is removed.
assert Plain.a == 5
assert Plain.b == 7
try:
    Bag.xs
    assert False, 'expected a factory field to leave no class attribute'
except AttributeError as e:
    assert str(e) == "type object 'Bag' has no attribute 'xs'"


# === A factory is called once per construction, in field order ===
calls = []


def track_a():
    calls.append('a')
    return 1


def track_b():
    calls.append('b')
    return 2


@dataclass
class Order:
    a: int = field(default_factory=track_a)
    b: int = field(default_factory=track_b)


assert Order() == Order(1, 2)
assert calls == ['a', 'b']
calls.clear()
# Only the unbound field's factory runs.
assert Order(a=9).a == 9
assert calls == ['b']


# === A factory that raises propagates, and builds no instance ===
def boom():
    raise ValueError('no default for you')


@dataclass
class Bad:
    x: int = field(default_factory=boom)


try:
    Bad()
    assert False, 'expected the factory error to propagate'
except ValueError as e:
    assert str(e) == 'no default for you'
# The field is still bindable explicitly, so the class itself is fine.
assert Bad(3).x == 3


# === field() rejects both a default and a factory ===
try:
    field(default=1, default_factory=list)
    assert False, 'expected both-given to raise'
except ValueError as e:
    assert str(e) == 'cannot specify both default and default_factory'


# === MISSING is a singleton compared by identity ===
assert MISSING is MISSING
# It is what an unset default or factory reads as, on both kinds of field.
assert Plain.__dataclass_fields__['a'].default == 5
assert Plain.__dataclass_fields__['a'].default_factory is MISSING
assert Bag.__dataclass_fields__['xs'].default is MISSING
assert Bag.__dataclass_fields__['xs'].default_factory is list


# === field() returns the Field the decorator then adopts ===
spec = field(default=3)
assert spec.name is None
assert spec.type is None


@dataclass
class Adopts:
    x: int = spec


assert Adopts.__dataclass_fields__['x'] is spec
assert spec.name == 'x'
assert spec.type == 'int' or spec.type is int
assert Adopts().x == 3


# === A factory field still follows the non-default ordering rule ===
@dataclass
class Ordering:
    required: int
    optional: list[int] = field(default_factory=list)


assert Ordering(1).optional == []
try:

    @dataclass
    class BadOrder:
        first: list[int] = field(default_factory=list)
        second: int

    assert False, 'expected a non-default after a default to raise'
except TypeError as e:
    assert str(e) == "non-default argument 'second' follows default argument 'first'"


# === A mutable default is still rejected; the factory is the way round it ===
try:

    @dataclass
    class Mutable:
        xs: list[int] = []

    assert False, 'expected a mutable default to raise'
except ValueError as e:
    assert str(e) == "mutable default <class 'list'> for field xs is not allowed: use default_factory"


# === __post_init__ runs once the fields are in place ===
@dataclass
class Derived:
    a: int
    doubled: int = 0

    def __post_init__(self):
        self.doubled = self.a * 2


d = Derived(5)
assert d.doubled == 10
assert repr(d) == 'Derived(a=5, doubled=10)'
# An explicitly passed value is still overwritten, as CPython's ordering implies.
assert Derived(5, 99).doubled == 10


# === __post_init__ sees factory-built fields ===
@dataclass
class Seeded:
    xs: list[int] = field(default_factory=list)

    def __post_init__(self):
        self.xs.append('seeded')


assert Seeded().xs == ['seeded']
assert Seeded(['given']).xs == ['given', 'seeded']


# === __post_init__ raising propagates out of the constructor ===
@dataclass
class Validated:
    a: int

    def __post_init__(self):
        if self.a < 0:
            raise ValueError('a must be non-negative')


assert Validated(1).a == 1
try:
    Validated(-1)
    assert False, 'expected __post_init__ to reject'
except ValueError as e:
    assert str(e) == 'a must be non-negative'


# === __post_init__ works with frozen=True, which its own assignment cannot ===
@dataclass(frozen=True)
class Frozen:
    a: int

    def __post_init__(self):
        # Reads are fine; an assignment here would raise, as in CPython.
        assert self.a == 1


assert Frozen(1).a == 1


# === A missing argument is reported before any factory runs ===
# CPython's generated `__init__` takes the factory sentinel as a parameter
# default, so argument binding raises before the body — and its factories — run.
calls = []


def record():
    calls.append(1)
    return 1


@dataclass
class Ordered:
    a: int
    b: int = field(default_factory=record)


try:
    Ordered()
    assert False, 'expected the missing argument to raise'
except TypeError as e:
    assert str(e) == "Ordered.__init__() missing 1 required positional argument: 'a'"
assert calls == []
# The factory still runs once the call is valid.
assert Ordered(0).b == 1
assert calls == [1]


# === A factory that rebinds __dataclass_fields__ cannot disturb later fields ===
# Every default and factory is snapshotted before the first factory runs, so a
# factory re-entering the VM sees no effect on the fields still to be filled.
def rebind():
    Rebinds.__dataclass_fields__ = {}
    return 'first'


@dataclass
class Rebinds:
    a: str = field(default_factory=rebind)
    b: list[int] = field(default_factory=list)


r = Rebinds()
assert r.a == 'first'
assert r.b == []


# === __post_init__ is decided at decoration, not at construction ===
# CPython bakes the call into the generated `__init__`, so a hook attached to
# the class afterwards is never reached.
@dataclass
class Late:
    a: int = 1


def never(self):
    raise AssertionError('a late __post_init__ must not run')


Late.__post_init__ = never
assert Late().a == 1
