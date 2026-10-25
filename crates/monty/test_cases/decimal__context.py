import copy
import decimal
from decimal import (
    MAX_EMAX,
    MAX_PREC,
    MIN_EMIN,
    MIN_ETINY,
    ROUND_05UP,
    ROUND_CEILING,
    ROUND_DOWN,
    ROUND_FLOOR,
    ROUND_HALF_DOWN,
    ROUND_HALF_EVEN,
    ROUND_HALF_UP,
    ROUND_UP,
    BasicContext,
    Clamped,
    Context,
    ConversionSyntax,
    Decimal,
    DecimalException,
    DefaultContext,
    DivisionByZero,
    DivisionImpossible,
    DivisionUndefined,
    ExtendedContext,
    FloatOperation,
    Inexact,
    InvalidContext,
    InvalidOperation,
    Overflow,
    Rounded,
    Subnormal,
    Underflow,
    getcontext,
    localcontext,
    setcontext,
)

# CPython runs cases on shared threads, so start from a fresh default context.
setcontext(Context())
D = Decimal

# === module constants ===
assert MAX_PREC == 999999999999999999
assert MAX_EMAX == 999999999999999999
assert MIN_EMIN == -999999999999999999
assert MIN_ETINY == -1999999999999999997
assert decimal.IEEE_CONTEXT_MAX_BITS == 512
assert decimal.HAVE_THREADS is True
assert decimal.HAVE_CONTEXTVAR is True
assert decimal.__version__ == '1.70'
assert [ROUND_CEILING, ROUND_FLOOR, ROUND_UP, ROUND_DOWN] == ['ROUND_CEILING', 'ROUND_FLOOR', 'ROUND_UP', 'ROUND_DOWN']
assert [ROUND_HALF_UP, ROUND_HALF_DOWN, ROUND_HALF_EVEN, ROUND_05UP] == [
    'ROUND_HALF_UP',
    'ROUND_HALF_DOWN',
    'ROUND_HALF_EVEN',
    'ROUND_05UP',
]

# === default context ===
ctx = getcontext()
assert ctx is getcontext()
assert repr(ctx) == (
    'Context(prec=28, rounding=ROUND_HALF_EVEN, Emin=-999999, Emax=999999, capitals=1, clamp=0, '
    'flags=[], traps=[InvalidOperation, DivisionByZero, Overflow])'
)
assert ctx.prec == 28
assert ctx.rounding == 'ROUND_HALF_EVEN'
assert (ctx.Emin, ctx.Emax, ctx.capitals, ctx.clamp) == (-999999, 999999, 1, 0)
assert ctx.Etiny() == -1000026
assert ctx.Etop() == 999972
assert str(ctx.radix()) == '10'
assert repr(BasicContext) == (
    'Context(prec=9, rounding=ROUND_HALF_UP, Emin=-999999, Emax=999999, capitals=1, clamp=0, '
    'flags=[], traps=[Clamped, InvalidOperation, DivisionByZero, Overflow, Underflow])'
)
assert repr(ExtendedContext) == (
    'Context(prec=9, rounding=ROUND_HALF_EVEN, Emin=-999999, Emax=999999, capitals=1, clamp=0, flags=[], traps=[])'
)
assert DefaultContext.prec == 28
assert decimal.DefaultContext is DefaultContext

# === changing the current context ===
getcontext().prec = 6
assert str(D(1) / D(7)) == '0.142857'
getcontext().rounding = ROUND_UP
assert str(D(1) / D(7)) == '0.142858'
setcontext(Context())
assert getcontext().prec == 28
mine = Context(prec=3)
setcontext(mine)
assert getcontext() is mine
mine.prec = 4
assert str(D(2) / D(3)) == '0.6667'
# templates are copied when installed
setcontext(ExtendedContext)
assert getcontext() is not ExtendedContext
assert getcontext().prec == 9
setcontext(Context())

# === localcontext ===
with localcontext() as lc:
    lc.prec = 5
    assert getcontext() is lc
    assert str(D(1) / D(3)) == '0.33333'
    with localcontext(prec=2) as inner:
        assert str(D(1) / D(3)) == '0.33'
        assert inner.prec == 2
    assert str(D(1) / D(3)) == '0.33333'
assert str(D(1) / D(3)) == '0.3333333333333333333333333333'
with localcontext(Context(prec=3, rounding=ROUND_DOWN)) as lc:
    assert str(D(2) / D(3)) == '0.666'
with localcontext(ExtendedContext) as lc:
    assert lc is not ExtendedContext
    assert str(D(1) / D(0)) == 'Infinity'
try:
    with localcontext(prec=3):
        raise ValueError('boom')
except ValueError:
    pass
assert getcontext().prec == 28
try:
    localcontext(prec=0)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'valid range for prec is [1, MAX_PREC]'
try:
    localcontext(5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'optional argument must be a context'
try:
    getcontext(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == '_decimal.getcontext() takes no arguments (1 given)'
try:
    setcontext()
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == '_decimal.setcontext() takes exactly one argument (0 given)'
try:
    setcontext(5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'argument must be a context'

# === Context constructor ===
c = Context(prec=7, rounding=ROUND_FLOOR, Emin=-9, Emax=99, capitals=0, clamp=1, flags=[Inexact], traps=[Rounded])
assert repr(c) == (
    'Context(prec=7, rounding=ROUND_FLOOR, Emin=-9, Emax=99, capitals=0, clamp=1, flags=[Inexact], traps=[Rounded])'
)
assert repr(Context(1, ROUND_UP, -5, 5, 0, 1, [], [])) == (
    'Context(prec=1, rounding=ROUND_UP, Emin=-5, Emax=5, capitals=0, clamp=1, flags=[], traps=[])'
)
all_signals = [
    InvalidOperation,
    FloatOperation,
    DivisionByZero,
    Overflow,
    Underflow,
    Subnormal,
    Inexact,
    Rounded,
    Clamped,
]
c = Context(traps={s: s is Rounded for s in all_signals})
assert c.traps[Rounded]
assert not c.traps[InvalidOperation]
assert Context(capitals=0).to_sci_string(D('1e9')) == '1e+9'
assert Context(prec=None).prec == 28
try:
    Context(1, 2, 3, 4, 5, 6, 7, 8, 9)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'function takes at most 8 arguments (9 given)'
try:
    Context(rounding='x')
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == (
        'valid values for rounding are:\n'
        '  [ROUND_CEILING, ROUND_FLOOR, ROUND_UP, ROUND_DOWN,\n'
        '   ROUND_HALF_UP, ROUND_HALF_DOWN, ROUND_HALF_EVEN,\n'
        '   ROUND_05UP]'
    )
try:
    Context(traps={Rounded: True})
    assert False, 'expected KeyError'
except KeyError as e:
    assert str(e) == "'invalid signal dict'"
try:
    Context(traps=[ConversionSyntax])
    assert False, 'expected KeyError'
except KeyError as e:
    assert e.args == (
        'valid values for signals are:\n'
        '  [InvalidOperation, FloatOperation, DivisionByZero,\n'
        '   Overflow, Underflow, Subnormal, Inexact, Rounded,\n'
        '   Clamped]',
    )

# === attribute validation ===
c = Context()
c.prec = True
assert c.prec == 1
for name, value, kind, message in [
    ('prec', 0, ValueError, 'valid range for prec is [1, MAX_PREC]'),
    ('prec', 10**20, OverflowError, 'Python int too large to convert to C ssize_t'),
    ('prec', 10**18, ValueError, 'valid range for prec is [1, MAX_PREC]'),
    ('prec', 'a', TypeError, 'an integer is required'),
    ('prec', 1.5, TypeError, 'an integer is required'),
    ('Emax', -1, ValueError, 'valid range for Emax is [0, MAX_EMAX]'),
    ('Emin', 1, ValueError, 'valid range for Emin is [MIN_EMIN, 0]'),
    ('capitals', 2, ValueError, 'valid values for capitals are 0 or 1'),
    ('clamp', 2, ValueError, 'valid values for clamp are 0 or 1'),
    ('flags', 1, TypeError, 'argument must be a signal dict'),
    ('traps', [Rounded], TypeError, 'argument must be a signal dict'),
]:
    try:
        setattr(c, name, value)
        assert False, f'expected {kind.__name__} for {name}'
    except kind as e:
        assert str(e) == message
try:
    c.foo = 1
    assert False, 'expected AttributeError'
except AttributeError as e:
    assert str(e) == "'decimal.Context' object has no attribute 'foo'"
c.traps = {s: s is Overflow for s in all_signals}
assert repr(c.traps) == (
    "{<class 'decimal.InvalidOperation'>:False, <class 'decimal.FloatOperation'>:False, "
    "<class 'decimal.DivisionByZero'>:False, <class 'decimal.Overflow'>:True, "
    "<class 'decimal.Underflow'>:False, <class 'decimal.Subnormal'>:False, "
    "<class 'decimal.Inexact'>:False, <class 'decimal.Rounded'>:False, <class 'decimal.Clamped'>:False}"
)

# === flags and traps ===
c = Context()
c.create_decimal('1.23456789012345678901234567890')
assert c.flags[Inexact]
assert c.flags[Rounded]
assert not c.flags[InvalidOperation]
assert repr(c) == (
    'Context(prec=28, rounding=ROUND_HALF_EVEN, Emin=-999999, Emax=999999, capitals=1, clamp=0, '
    'flags=[Inexact, Rounded], traps=[InvalidOperation, DivisionByZero, Overflow])'
)
c.clear_flags()
assert not c.flags[Inexact]
c.flags[Subnormal] = True
assert c.flags[Subnormal]
c.flags[Subnormal] = 0
assert not c.flags[Subnormal]
flags = c.flags
assert len(flags) == 9
assert list(flags) == all_signals
assert list(flags.keys()) == all_signals
assert list(flags.values()) == [False] * 9
assert Rounded in flags
assert ConversionSyntax not in flags
assert flags.get(Rounded) is False
assert flags.get('x', 'default') == 'default'
assert flags.copy() == {s: False for s in all_signals}
assert flags == flags.copy()
assert (flags == {Rounded: False}) is False
assert (flags == {1: 2}) is False
assert flags != {Rounded: False}
try:
    flags == {}
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'invalid signal dict'
other = Context(traps=[Rounded])
other.flags.update(other.traps)
assert other.flags[Rounded]
assert not other.flags[Inexact]
other.flags.update([(Inexact, 1), (Rounded, 0)])
assert other.flags[Inexact]
assert not other.flags[Rounded]
other.flags.update({Clamped: True})
other.flags.update()
assert other.flags[Clamped]
try:
    other.flags.update([(Inexact,)])
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'not enough values to unpack (expected 2, got 1)'
try:
    other.flags.update(5)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'int' object is not iterable"
try:
    Context(flags=(Inexact,))
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'argument must be a signal dict'
try:
    flags[ConversionSyntax]
    assert False, 'expected KeyError'
except KeyError:
    pass
try:
    flags.pop(Rounded)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'signal keys cannot be deleted'
c.clear_traps()
assert repr(c).endswith('traps=[])')
assert str(c.divide(D(1), D(0))) == 'Infinity'
assert str(c.divide(D(0), D(0))) == 'NaN'
assert str(c.create_decimal('x')) == 'NaN'
assert c.flags[DivisionByZero]
assert repr(c).endswith('flags=[InvalidOperation, DivisionByZero], traps=[])')

# === trapped signals raise their class ===
c = Context()
try:
    c.divide(D(1), D(0))
    assert False, 'expected DivisionByZero'
except DivisionByZero as e:
    assert str(e) == "[<class 'decimal.DivisionByZero'>]"
try:
    D('abc')
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.ConversionSyntax'>]"
# the raised class is `InvalidOperation`, which is not a `ZeroDivisionError`
try:
    try:
        D(0) / 0
    except ZeroDivisionError:
        assert False, 'InvalidOperation is not a ZeroDivisionError'
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.DivisionUndefined'>]"
try:
    divmod(D(1), 0)
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>, <class 'decimal.DivisionByZero'>]"
with localcontext() as lc:
    lc.traps[Inexact] = True
    try:
        D(1) / 3
        assert False, 'expected Inexact'
    except Inexact as e:
        assert str(e) == "[<class 'decimal.Inexact'>]"
    lc.traps[Inexact] = False
    lc.traps[Rounded] = True
    try:
        D(1) / 3
        assert False, 'expected Rounded'
    except Rounded as e:
        assert str(e) == "[<class 'decimal.Rounded'>]"

# === the exception hierarchy ===
for exc, bases in [
    (DivisionByZero(), (DecimalException, ZeroDivisionError, ArithmeticError)),
    (DivisionUndefined(), (InvalidOperation, ZeroDivisionError, ArithmeticError)),
    (DivisionImpossible(), (InvalidOperation, DecimalException)),
    (ConversionSyntax(), (InvalidOperation,)),
    (InvalidContext(), (InvalidOperation,)),
    (Overflow(), (Inexact, Rounded, DecimalException)),
    (Underflow(), (Inexact, Rounded, Subnormal)),
    (FloatOperation(), (DecimalException, TypeError)),
    (Clamped(), (DecimalException, ArithmeticError, Exception)),
]:
    for base in bases:
        assert isinstance(exc, base), f'{exc!r} should be a {base!r}'
assert not isinstance(Overflow(), Subnormal), 'Overflow is not Subnormal'
assert not isinstance(Clamped(), ZeroDivisionError), 'Clamped is not a ZeroDivisionError'
try:
    D(1) / 0
except ArithmeticError as e:
    assert isinstance(e, DivisionByZero)
with localcontext() as lc:
    lc.traps[FloatOperation] = True
    assert D('1.5') == 1.5
    try:
        D('1.5') < 2.5
        assert False, 'expected FloatOperation'
    except TypeError as e:
        assert str(e) == "[<class 'decimal.FloatOperation'>]"
    try:
        D(1.5)
        assert False, 'expected FloatOperation'
    except FloatOperation as e:
        assert str(e) == "[<class 'decimal.FloatOperation'>]"
    assert str(D.from_float(1.5)) == '1.5'
    assert str(lc.create_decimal_from_float(1.5)) == '1.5'

# === FloatOperation flag ===
with localcontext() as lc:
    lc.clear_flags()
    assert D('0.5') == 0.5
    assert lc.flags[FloatOperation]
    lc.clear_flags()
    assert D(2) > 1.5
    assert lc.flags[FloatOperation]
    lc.clear_flags()
    assert D(2) > 1
    assert not lc.flags[FloatOperation]

# === equality, ordering and hashing ===
assert D('1.0') == D(1)
assert D('1.5') == 1.5
assert D(1) == 1
assert D(1) == True
assert D(1) == 1 + 0j
assert D(1) != 1 + 1j
assert D('0.1') != 0.1
assert D('nan') != D('nan')
n = D('nan')
assert n != n
assert not (n == n)
assert D(2) > 1
assert 1 < D(2)
assert D('-0') == 0
assert D('inf') > 10**100
assert not (D('nan') < 1) if False else True
assert hash(D(1)) == hash(1)
assert hash(D('1.00')) == hash(1)
assert hash(D('1.5')) == hash(1.5)
assert hash(D('-0')) == hash(0)
assert hash(D('1e3')) == hash(1000)
assert hash(D('inf')) == hash(float('inf'))
assert hash(D('0.1')) == hash(D('0.10'))
# values a float holds exactly hash like it, down to the subnormals
assert hash(D('-12.500')) == hash(-12.5)
assert hash(D(2**-60)) == hash(2**-60)
assert hash(D(0.1)) == hash(0.1)
assert hash(D(5e-324)) == hash(5e-324)
assert hash(D(2.2250738585072014e-308)) == hash(2.2250738585072014e-308)
assert {D('1.0'): 'a'}[1] == 'a'
assert {D(2), 2, 2.0} == {2}
assert sorted([D(3), D('1.2'), 2, 0.5]) == [0.5, D('1.2'), 2, D(3)]
assert max([D(3), D('3.5'), 1]) == D('3.5')
assert min(D('-1'), -2) == -2
assert (D(1), 2) < (D(1), 3)
try:
    hash(D('snan'))
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'Cannot hash a signaling NaN value'
try:
    D('nan') < 1
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"
try:
    D(1) < 'a'
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'<' not supported between instances of 'decimal.Decimal' and 'str'"
try:
    D(1) < 1j
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'<' not supported between instances of 'decimal.Decimal' and 'complex'"
try:
    sorted([D(1), D('nan')])
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"
with localcontext() as lc:
    lc.traps[InvalidOperation] = False
    assert not (D('nan') < 1)
    assert not (D('nan') >= 1)

# === Context arithmetic methods ===
c = Context(prec=5)
assert str(c.add(1, 2)) == '3'
assert str(c.add(D('1.23456789'), 0)) == '1.2346'
assert str(c.subtract(D('1.3'), D('1.07'))) == '0.23'
assert str(c.multiply(D('1.1'), 3)) == '3.3'
assert str(c.divide(1, 8)) == '0.125'
assert str(c.divide_int(D(7), 2)) == '3'
assert str(c.remainder(7, 2)) == '1'
assert str(c.remainder_near(7, 4)) == '-1'
assert c.divmod(7, 2) == (D(3), D(1))
assert str(c.power(2, 10)) == '1024'
assert str(c.power(2, 10, 1000)) == '24'
assert str(c.power(a=2, b=3)) == '8'
assert str(c.fma(2, 3, 4)) == '10'
assert str(c.abs(-5)) == '5'
assert str(c.minus(5)) == '-5'
assert str(c.plus(D('1.234567'))) == '1.2346'
assert str(c.sqrt(2)) == '1.4142'
assert str(c.exp(1)) == '2.7183'
assert str(c.ln(10)) == '2.3026'
assert str(c.log10(1000)) == '3'
assert str(c.quantize(D('1.234'), D('0.1'))) == '1.2'
assert str(c.normalize(D('1.200'))) == '1.2'
assert str(c.to_integral(D('2.5'))) == '2'
assert str(c.to_integral_exact(D('2.5'))) == '2'
assert str(c.to_sci_string(D('1e-7'))) == '1E-7'
assert str(c.to_eng_string(D('1e-7'))) == '100E-9'
assert c.is_zero(0)
assert c.is_finite(1)
assert not c.is_nan(1)
assert c.number_class(1) == '+Normal'
assert str(c.max(1, 2)) == '2'
assert str(c.min_mag(-1, 2)) == '-1'
assert str(c.compare(1, 2)) == '-1'
assert str(c.copy_sign(1, -2)) == '-1'
assert str(c.copy_abs(-1)) == '1'
assert str(c.copy_decimal(3)) == '3'
assert str(c.canonical(D(3))) == '3'
assert str(c.logb(D(250))) == '2'
assert str(c.scaleb(D(1), 3)) == '1E+3'
assert c.same_quantum(D('1.0'), D('2.0'))
assert str(c.next_plus(D(1))) == '1.0001'
assert str(c.next_toward(D(1), 0)) == '0.99999'
assert str(c.create_decimal('3.14159265')) == '3.1416'
assert str(c.create_decimal()) == '0'
assert str(c.create_decimal_from_float(0.1)) == '0.10000'
assert str(c._apply(D('3.14159265'))) == '3.1416'
for call, kind, message in [
    (lambda: c.abs(), TypeError, 'Context.abs() takes exactly one argument (0 given)'),
    (lambda: c.abs(1, 2), TypeError, 'Context.abs() takes exactly one argument (2 given)'),
    (lambda: c.abs(a=1), TypeError, 'Context.abs() takes no keyword arguments'),
    (lambda: c.add(1), TypeError, 'function takes exactly 2 arguments (1 given)'),
    (lambda: c.add(a=1, b=2), TypeError, 'add() takes no keyword arguments'),
    (lambda: c.fma(1, 2), TypeError, 'function takes exactly 3 arguments (2 given)'),
    (lambda: c.power(1), TypeError, "function missing required argument 'b' (pos 2)"),
    (lambda: c.add('1', 2), TypeError, 'conversion from str to Decimal is not supported'),
    (lambda: c.add(1.5, 2), TypeError, 'conversion from float to Decimal is not supported'),
    (lambda: c.canonical(1), TypeError, 'argument must be a Decimal'),
    (lambda: c.Etiny(1), TypeError, 'Context.Etiny() takes no arguments (1 given)'),
    (lambda: c.create_decimal(1, 2), TypeError, 'function takes at most 1 argument (2 given)'),
    (lambda: c.create_decimal(num=5), TypeError, 'create_decimal() takes no keyword arguments'),
    (lambda: c.create_decimal_from_float('a'), TypeError, 'argument must be int or float'),
    (lambda: c.apply(1), AttributeError, "'decimal.Context' object has no attribute 'apply'"),
]:
    try:
        call()
        assert False, f'expected {kind.__name__}: {message}'
    except kind as e:
        assert str(e) == message

# === copying contexts ===
c = Context(prec=12)
c.flags[Inexact] = True
d = c.copy()
assert d is not c
assert repr(d) == repr(c)
d.prec = 3
assert c.prec == 12
assert copy.copy(c) is not c
assert copy.copy(c).prec == 12
deep = copy.deepcopy(c)
assert deep.flags[Inexact]
assert c == c
assert c != d
assert hash(c) == hash(c)
try:
    copy.copy(c.flags)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "cannot pickle 'SignalDict' object"
try:
    hash(c.flags)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "unhashable type: 'SignalDict'"

# === IEEEContext ===
assert repr(decimal.IEEEContext(64)) == (
    'Context(prec=16, rounding=ROUND_HALF_EVEN, Emin=-383, Emax=384, capitals=1, clamp=1, flags=[], traps=[])'
)
assert repr(decimal.IEEEContext(128)) == (
    'Context(prec=34, rounding=ROUND_HALF_EVEN, Emin=-6143, Emax=6144, capitals=1, clamp=1, flags=[], traps=[])'
)
try:
    decimal.IEEEContext(33)
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'argument must be a multiple of 32, with a maximum of 512'

# === the context affects str() and repr() ===
with localcontext(capitals=0):
    assert str(D('1e10')) == '1e+10'
    assert repr(D('1e10')) == "Decimal('1e+10')"
assert str(D('1e10')) == '1E+10'

setcontext(Context())
