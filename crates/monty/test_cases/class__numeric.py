import math
from datetime import timedelta

# === Unary and rounding hooks preserve arbitrary return values ===


class Number:
    def __abs__(self):
        return self.result

    def __invert__(self):
        return self.result

    def __round__(self, *args):
        self.args = args
        return self.result

    def __floor__(self):
        return self.result

    def __ceil__(self):
        return self.result

    def __trunc__(self):
        return self.result


number = Number()
for result in [None, NotImplemented, ['result'], number]:
    number.result = result
    assert abs(number) is result
    assert ~number is result
    assert round(number) is result
    assert number.args == ()
    assert round(number, None) is result
    assert number.args == ()
    assert round(number, 'raw precision') is result
    assert number.args == ('raw precision',)
    assert math.floor(number) is result
    assert math.ceil(number) is result
    assert math.trunc(number) is result

# Instance attributes never shadow numeric special methods.
number.__abs__ = None
number.__invert__ = None
number.__round__ = None
number.__floor__ = None
number.__ceil__ = None
number.__trunc__ = None
number.result = ['class method']
assert abs(number) is number.result
assert ~number is number.result
assert round(number, number) is number.result
assert number.args == (number,)
assert math.floor(number) is number.result
assert math.ceil(number) is number.result
assert math.trunc(number) is number.result

# === Integer and float conversion precedence ===
calls = []


class Convertible:
    def __int__(self):
        calls.append('int')
        return self.integer

    def __float__(self):
        calls.append('float')
        return self.real

    def __index__(self):
        calls.append('index')
        return 99


convertible = Convertible()
convertible.integer = 42
convertible.real = 2.5
convertible.__int__ = None
convertible.__float__ = None
convertible.__index__ = None
assert int(convertible) == 42
assert float(convertible) == 2.5
assert calls == ['int', 'float']
assert math.floor(convertible) == 2
assert math.ceil(convertible) == 3
assert calls == ['int', 'float', 'float', 'float']

for integer in [True, False, 2**100, -(2**100)]:
    convertible.integer = integer
    assert int(convertible) == int(integer)
    assert type(int(convertible)) is int

for bad in ['bad', [], None, NotImplemented, convertible]:
    convertible.integer = bad
    convertible.real = bad
    calls.clear()
    try:
        int(convertible)
        assert False, 'expected __int__ result validation'
    except TypeError as exc:
        assert str(exc) == f'__int__ returned non-int (type {type(bad).__name__})'
    try:
        float(convertible)
        assert False, 'expected __float__ result validation'
    except TypeError as exc:
        assert str(exc) == f'Convertible.__float__ returned non-float (type {type(bad).__name__})'
    assert calls == ['int', 'float']

convertible.real = 1
try:
    float(convertible)
    assert False, 'expected an integer __float__ result to fail'
except TypeError as exc:
    assert str(exc) == 'Convertible.__float__ returned non-float (type int)'

# Rounding hooks take precedence over float and index conversion.
for name, operation in [('__floor__', math.floor), ('__ceil__', math.ceil), ('__trunc__', math.trunc)]:
    Rounded = type('Rounded', (), {name: lambda self: ['rounded'], '__float__': lambda self: 9.5})
    assert operation(Rounded()) == ['rounded']

# Builtins and already-bound methods used as hooks are not rebound.
number = Number()
number.result = ['bound result']


class BoundConversions:
    __abs__ = number.__abs__
    __invert__ = number.__invert__
    __round__ = number.__round__
    __int__ = int
    __float__ = float
    __index__ = int
    __floor__ = number.__floor__
    __ceil__ = number.__ceil__
    __trunc__ = number.__trunc__


bound = BoundConversions()
assert abs(bound) is number.result
assert ~bound is number.result
assert round(bound, ['precision']) is number.result
assert number.args == (['precision'],)
assert int(bound) == 0
assert float(bound) == 0.0
assert bin(bound) == '0b0'
assert math.floor(bound) is number.result
assert math.ceil(bound) is number.result
assert math.trunc(bound) is number.result

# === Index fallback preserves arbitrarily large integers ===


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        return self.value


for integer in [0, 65, -1, 2**100, -(2**100), True]:
    index = Index(integer)
    assert int(index) == int(integer)
    assert type(int(index)) is int
    assert float(index) == float(integer)
    assert bin(index) == bin(integer)
    assert oct(index) == oct(integer)
    assert hex(index) == hex(integer)
    assert math.floor(index) == math.floor(float(integer))
    assert math.ceil(index) == math.ceil(float(integer))

assert chr(Index(65)) == 'A'
assert chr(Index(True)) == '\x01'
assert int('101', Index(2)) == 5
assert int('0xff', Index(0)) == 255
assert round(1234, Index(-2)) == 1200
assert round(1.25, Index(1)) == 1.2
assert round(10**30, Index(-29)) == 10**30
assert round(1234, Index(10**30)) == 1234
assert round(1234, Index(-1000)) == 0
assert round(1.25, Index(10**30)) == 1.25
assert round(-1.25, Index(-(10**30))) == 0.0
assert math.copysign(1.0, round(-1.25, Index(-(10**30)))) == -1.0

for value in [-1, 0x110000, 2**100, -(2**100)]:
    try:
        chr(Index(value))
        assert False, 'expected out-of-range character'
    except ValueError as exc:
        assert str(exc) == 'chr() arg not in range(0x110000)'

for value in [1, 37, 2**100, -(2**100)]:
    try:
        int('10', Index(value))
        assert False, 'expected invalid base'
    except ValueError as exc:
        assert str(exc) == 'int() base must be >= 2 and <= 36, or 0'

try:
    float(Index(10**1000))
    assert False, 'expected float overflow'
except OverflowError as exc:
    assert str(exc) == 'int too large to convert to float'

# === Missing, non-callable and raising hooks ===
operations = [
    ('__abs__', abs, "bad operand type for abs(): 'Missing'"),
    ('__invert__', lambda x: ~x, "bad operand type for unary ~: 'Missing'"),
    ('__round__', round, "type Missing doesn't define __round__ method"),
    ('__int__', int, "int() argument must be a string, a bytes-like object or a real number, not 'Missing'"),
    ('__float__', float, "float() argument must be a string or a real number, not 'Missing'"),
    ('__floor__', math.floor, 'must be real number, not Missing'),
    ('__ceil__', math.ceil, 'must be real number, not Missing'),
    ('__trunc__', math.trunc, "type Missing doesn't define __trunc__ method"),
]


class Missing:
    pass


def fail(self):
    raise ValueError('numeric hook failed')


for name, operation, message in operations:
    missing = Missing()
    setattr(missing, name, lambda: 1)
    try:
        operation(missing)
        assert False, 'expected class-only lookup'
    except TypeError as exc:
        assert str(exc) == message

    Disabled = type('Disabled', (), {name: None})
    try:
        operation(Disabled())
        assert False, 'expected non-callable hook'
    except TypeError as exc:
        assert str(exc) == "'NoneType' object is not callable"

    Raising = type('Raising', (), {name: fail})
    try:
        operation(Raising())
        assert False, 'expected hook exception'
    except ValueError as exc:
        assert str(exc) == 'numeric hook failed'

# No __trunc__ fallback from int(), nor index/float fallback from trunc().
number.result = 7
try:
    int(number)
    assert False, 'expected no __trunc__ fallback'
except TypeError as exc:
    assert str(exc) == "int() argument must be a string, a bytes-like object or a real number, not 'Number'"

try:
    math.trunc(Index(2))
    assert False, 'expected no __index__ fallback from trunc'
except TypeError as exc:
    assert str(exc) == "type Index doesn't define __trunc__ method"

# Missing __round__ wins over coercion of ndigits.
try:
    round(Missing(), [])
    assert False, 'expected missing __round__'
except TypeError as exc:
    assert str(exc) == "type Missing doesn't define __round__ method"

# Every new index consumer validates results and propagates exceptions.
index_consumers = [
    int,
    float,
    bin,
    oct,
    hex,
    chr,
    math.floor,
    math.ceil,
    lambda x: int('10', x),
    lambda x: round(1.5, x),
    lambda x: round(12, x),
]
for operation in index_consumers:
    try:
        operation(Index([]))
        assert False, 'expected invalid __index__ result'
    except TypeError as exc:
        assert str(exc) == '__index__ returned non-int (type list)'
    DisabledIndex = type('DisabledIndex', (), {'__index__': None})
    try:
        operation(DisabledIndex())
        assert False, 'expected non-callable __index__'
    except TypeError as exc:
        assert str(exc) == "'NoneType' object is not callable"
    RaisingIndex = type('RaisingIndex', (), {'__index__': fail})
    try:
        operation(RaisingIndex())
        assert False, 'expected index exception'
    except ValueError as exc:
        assert str(exc) == 'numeric hook failed'

# === Builtin implementations retain numeric semantics ===
assert abs(-(2**63)) == 2**63
assert abs(-(2**100)) == 2**100
assert abs(True) == 1
assert type(abs(True)) is int
assert math.copysign(1.0, abs(-0.0)) == 1.0
assert abs(3 + 4j) == 5.0
assert abs(timedelta(days=-2, seconds=3)) == timedelta(days=1, seconds=86397)
assert ~0 == -1
assert ~(2**100) == -(2**100) - 1
assert ~(-(2**100)) == 2**100 - 1
assert round(2**63 - 1, -1) == 9223372036854775810
assert round(True) == 1
assert type(round(True)) is int
for integer in [True, 1, -1, 2**100, -(2**100)]:
    assert math.floor(integer) == integer
    assert math.ceil(integer) == integer
    assert math.trunc(integer) == integer
    assert type(math.floor(integer)) is int
    assert type(math.ceil(integer)) is int
    assert type(math.trunc(integer)) is int
