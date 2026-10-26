# The `KW_ONLY` marker: a pseudo-field whose annotation makes every field after
# it keyword-only. Detected textually, like `ClassVar` (see
# limitations/dataclasses.md).
from dataclasses import KW_ONLY, dataclass, field


# === Fields after the marker are keyword-only; the marker is not a field ===
@dataclass
class Split:
    a: int
    _: KW_ONLY
    b: int
    c: int = 3


assert list(Split.__dataclass_fields__) == ['a', 'b', 'c']
assert Split(1, b=2) == Split(a=1, b=2, c=3)
assert not hasattr(Split, '_')
try:
    Split(1, 2)
    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == 'Split.__init__() takes 2 positional arguments but 3 were given'


# === The marker lifts the ordering rule for what follows it ===
@dataclass
class Reordered:
    a: int = 1
    _: KW_ONLY
    b: int


assert Reordered(b=2) == Reordered(a=1, b=2)


# === A field may still opt out of the marker ===
@dataclass
class OptOut:
    a: int
    _: KW_ONLY
    b: int = field(default=2, kw_only=False)


assert OptOut(1, 2) == OptOut(a=1, b=2)


# === Two markers are an error, naming the second ===
try:

    @dataclass
    class Twice:
        a: int
        _: KW_ONLY
        x: KW_ONLY
        b: int

    assert False, 'expected a TypeError'
except TypeError as e:
    assert str(e) == "'x' is KW_ONLY, but KW_ONLY has already been specified"


# === KW_ONLY is a plain object, printed with its id ===
assert repr(KW_ONLY).startswith('<dataclasses._KW_ONLY_TYPE object at 0x')
