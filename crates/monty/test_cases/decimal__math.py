import math
from decimal import (
    ROUND_CEILING,
    ROUND_DOWN,
    ROUND_FLOOR,
    ROUND_HALF_EVEN,
    ROUND_UP,
    Clamped,
    Context,
    Decimal,
    DecimalException,
    DivisionByZero,
    FloatOperation,
    Inexact,
    InvalidOperation,
    Overflow,
    Rounded,
    Subnormal,
    Underflow,
    getcontext,
    localcontext,
)

SIGNALS = [InvalidOperation, FloatOperation, DivisionByZero, Overflow, Underflow, Subnormal, Inexact, Rounded, Clamped]
NAMES = [
    'InvalidOperation',
    'FloatOperation',
    'DivisionByZero',
    'Overflow',
    'Underflow',
    'Subnormal',
    'Inexact',
    'Rounded',
    'Clamped',
]


def run(f):
    """Runs `f` with cleared flags; returns its repr (or `!` and the trap message) and the flags set."""
    c = getcontext()
    c.clear_flags()
    try:
        r = repr(f())
    except DecimalException as e:
        r = '!' + str(e)
    return r, [n for s, n in zip(SIGNALS, NAMES) if c.flags[s]]


# === Powers at the default context ===
assert run(lambda: Decimal(2) ** 10) == ("Decimal('1024')", [])
assert run(lambda: Decimal(2) ** -2) == ("Decimal('0.25')", [])
assert run(lambda: Decimal('1.5') ** 2) == ("Decimal('2.25')", [])
assert run(lambda: Decimal('1.10') ** 3) == ("Decimal('1.331000')", [])
assert run(lambda: Decimal(2) ** 100) == ("Decimal('1.267650600228229401496703205E+30')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal(3) ** Decimal('0.5')) == ("Decimal('1.732050807568877293527446342')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal(4) ** Decimal('0.5')) == ("Decimal('2.000000000000000000000000000')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal(27) ** (Decimal(1) / 3)) == (
    "Decimal('3.000000000000000000000000000')",
    ['Inexact', 'Rounded'],
)
assert run(lambda: Decimal('0.25') ** Decimal('-0.5')) == (
    "Decimal('2.000000000000000000000000000')",
    ['Inexact', 'Rounded'],
)
assert run(lambda: Decimal(10) ** -3) == ("Decimal('0.001')", [])
assert run(lambda: Decimal('1e10') ** 2) == ("Decimal('1E+20')", [])
assert run(lambda: Decimal('100') ** Decimal('1.5')) == (
    "Decimal('1000.000000000000000000000000')",
    ['Inexact', 'Rounded'],
)
assert run(lambda: Decimal(-2) ** 3) == ("Decimal('-8')", [])
assert run(lambda: Decimal(-2) ** 2) == ("Decimal('4')", [])
assert run(lambda: Decimal(-2) ** -3) == ("Decimal('-0.125')", [])
assert run(lambda: Decimal(-8) ** Decimal('0.5')) == ("![<class 'decimal.InvalidOperation'>]", ['InvalidOperation'])
assert run(lambda: Decimal('-0') ** Decimal('0.5')) == ("Decimal('0')", [])
assert run(lambda: Decimal(0) ** 0) == ("![<class 'decimal.InvalidOperation'>]", ['InvalidOperation'])
assert run(lambda: Decimal(0) ** 2) == ("Decimal('0')", [])
assert run(lambda: Decimal(0) ** -2) == ("Decimal('Infinity')", [])
assert run(lambda: Decimal('-0') ** -3) == ("Decimal('-Infinity')", [])
assert run(lambda: Decimal(5) ** 0) == ("Decimal('1')", [])
assert run(lambda: Decimal('inf') ** 0) == ("Decimal('1')", [])
assert run(lambda: Decimal('inf') ** 2) == ("Decimal('Infinity')", [])
assert run(lambda: Decimal('inf') ** -2) == ("Decimal('0')", [])
assert run(lambda: Decimal('-inf') ** 3) == ("Decimal('-Infinity')", [])
assert run(lambda: Decimal('-inf') ** 2) == ("Decimal('Infinity')", [])
assert run(lambda: Decimal(2) ** Decimal('inf')) == ("Decimal('Infinity')", [])
assert run(lambda: Decimal('0.5') ** Decimal('inf')) == ("Decimal('0')", [])
assert run(lambda: Decimal(2) ** Decimal('-inf')) == ("Decimal('0')", [])
assert run(lambda: Decimal(1) ** Decimal('inf')) == ("Decimal('1.000000000000000000000000000')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal('nan') ** 2) == ("Decimal('NaN')", [])
assert run(lambda: Decimal(2) ** Decimal('nan')) == ("Decimal('NaN')", [])
assert run(lambda: Decimal('snan') ** 2) == ("![<class 'decimal.InvalidOperation'>]", ['InvalidOperation'])
assert run(lambda: Decimal(1) ** 5) == ("Decimal('1')", [])
assert run(lambda: Decimal('1.0') ** 5) == ("Decimal('1.00000')", [])
assert run(lambda: Decimal('1.00') ** -2) == ("Decimal('1')", [])
assert run(lambda: Decimal('1.0') ** Decimal('0.5')) == (
    "Decimal('1.000000000000000000000000000')",
    ['Inexact', 'Rounded'],
)
assert run(lambda: Decimal('1.000') ** 100) == ("Decimal('1.000000000000000000000000000')", ['Rounded'])
assert run(lambda: Decimal(10) ** Decimal(10**7)) == (
    "![<class 'decimal.Overflow'>]",
    ['Overflow', 'Inexact', 'Rounded'],
)
assert run(lambda: Decimal('0.1') ** Decimal(10**7)) == (
    "Decimal('0E-1000026')",
    ['Underflow', 'Subnormal', 'Inexact', 'Rounded', 'Clamped'],
)
assert run(lambda: Decimal('1.000001') ** 1000000) == (
    "Decimal('2.718280469319376883819799708')",
    ['Inexact', 'Rounded'],
)
assert run(lambda: Decimal('2.5') ** Decimal('-1.5')) == (
    "Decimal('0.2529822128134703465599114836')",
    ['Inexact', 'Rounded'],
)
assert run(lambda: 2 ** Decimal('0.5')) == ("Decimal('1.414213562373095048801688724')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal(2) ** True) == ("Decimal('2')", [])
assert run(lambda: Decimal(7) ** Decimal('1e2')) == (
    "Decimal('3.234476509624757991344647769E+84')",
    ['Inexact', 'Rounded'],
)

# === Three-argument pow ===
assert run(lambda: pow(Decimal(2), 10, 1000)) == ("Decimal('24')", [])
assert run(lambda: pow(Decimal(3), 200, 7)) == ("Decimal('2')", [])
assert run(lambda: pow(Decimal(-3), 3, 7)) == ("Decimal('-6')", [])
assert run(lambda: pow(Decimal(3), 3, -7)) == ("Decimal('6')", [])
assert run(lambda: pow(2, Decimal(10), 1000)) == ("Decimal('24')", [])
assert run(lambda: pow(Decimal(2), Decimal('1e9'), 7)) == ("Decimal('2')", [])
assert run(lambda: pow(Decimal('2.5'), 2, 7)) == ("![<class 'decimal.InvalidOperation'>]", ['InvalidOperation'])
assert run(lambda: pow(Decimal(2), -1, 7)) == ("![<class 'decimal.InvalidOperation'>]", ['InvalidOperation'])
assert run(lambda: pow(Decimal(2), 3, 0)) == ("![<class 'decimal.InvalidOperation'>]", ['InvalidOperation'])
assert run(lambda: pow(Decimal(0), 0, 7)) == ("![<class 'decimal.InvalidOperation'>]", ['InvalidOperation'])
assert run(lambda: pow(Decimal(2), 3, Decimal('1e30'))) == (
    "![<class 'decimal.InvalidOperation'>]",
    ['InvalidOperation'],
)
assert run(lambda: pow(Decimal(2), 3, Decimal('nan'))) == ("Decimal('NaN')", [])
assert run(lambda: pow(Decimal('snan'), 3, 7)) == ("![<class 'decimal.InvalidOperation'>]", ['InvalidOperation'])
assert run(lambda: pow(Decimal(2), Decimal('inf'), 7)) == (
    "![<class 'decimal.InvalidOperation'>]",
    ['InvalidOperation'],
)
assert run(lambda: pow(Decimal('20E1'), 3, 7)) == ("Decimal('1')", [])
assert run(lambda: pow(Decimal(0), 5, 7)) == ("Decimal('0')", [])
assert run(lambda: Decimal(2).sqrt()) == ("Decimal('1.414213562373095048801688724')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal(4).sqrt()) == ("Decimal('2')", [])
assert run(lambda: Decimal('0.0100').sqrt()) == ("Decimal('0.10')", [])
assert run(lambda: Decimal(100).sqrt()) == ("Decimal('10')", [])
assert run(lambda: Decimal('1e7').sqrt()) == ("Decimal('3162.277660168379331998893544')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal('1e-7').sqrt()) == ("Decimal('0.0003162277660168379331998893544')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal(-1).sqrt()) == ("![<class 'decimal.InvalidOperation'>]", ['InvalidOperation'])
assert run(lambda: Decimal('-0').sqrt()) == ("Decimal('-0')", [])
assert run(lambda: Decimal('0.000').sqrt()) == ("Decimal('0.00')", [])
assert run(lambda: Decimal('inf').sqrt()) == ("Decimal('Infinity')", [])
assert run(lambda: Decimal('-inf').sqrt()) == ("![<class 'decimal.InvalidOperation'>]", ['InvalidOperation'])
assert run(lambda: Decimal('nan').sqrt()) == ("Decimal('NaN')", [])
assert run(lambda: Decimal('snan').sqrt()) == ("![<class 'decimal.InvalidOperation'>]", ['InvalidOperation'])
assert run(lambda: Decimal('1.21').sqrt()) == ("Decimal('1.1')", [])
assert run(lambda: Decimal(10).sqrt()) == ("Decimal('3.162277660168379331998893544')", ['Inexact', 'Rounded'])

# === sqrt, exp, ln and log10 ===
assert run(lambda: Decimal(1).exp()) == ("Decimal('2.718281828459045235360287471')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal(0).exp()) == ("Decimal('1')", [])
assert run(lambda: Decimal(-1).exp()) == ("Decimal('0.3678794411714423215955237702')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal(10).exp()) == ("Decimal('22026.46579480671651695790065')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal('0.001').exp()) == ("Decimal('1.001000500166708341668055754')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal('1e-30').exp()) == ("Decimal('1.000000000000000000000000000')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal('-1e-30').exp()) == ("Decimal('1.000000000000000000000000000')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal('inf').exp()) == ("Decimal('Infinity')", [])
assert run(lambda: Decimal('-inf').exp()) == ("Decimal('0')", [])
assert run(lambda: Decimal('nan').exp()) == ("Decimal('NaN')", [])
assert run(lambda: Decimal(1000000).exp()) == (
    "Decimal('3.033215396802087545086402141E+434294')",
    ['Inexact', 'Rounded'],
)
assert run(lambda: Decimal(-1000000).exp()) == (
    "Decimal('3.296831478088558578968907969E-434295')",
    ['Inexact', 'Rounded'],
)
assert run(lambda: Decimal(230).exp()) == ("Decimal('7.722018499983835717562125214E+99')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal(10).ln()) == ("Decimal('2.302585092994045684017991455')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal(1).ln()) == ("Decimal('0')", [])
assert run(lambda: Decimal('1.000').ln()) == ("Decimal('0')", [])
assert run(lambda: Decimal(0).ln()) == ("Decimal('-Infinity')", [])
assert run(lambda: Decimal(-1).ln()) == ("![<class 'decimal.InvalidOperation'>]", ['InvalidOperation'])
assert run(lambda: Decimal('inf').ln()) == ("Decimal('Infinity')", [])
assert run(lambda: Decimal('-inf').ln()) == ("![<class 'decimal.InvalidOperation'>]", ['InvalidOperation'])
assert run(lambda: Decimal('0.5').ln()) == ("Decimal('-0.6931471805599453094172321215')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal('1e100').ln()) == ("Decimal('230.2585092994045684017991455')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal('1e-100').ln()) == ("Decimal('-230.2585092994045684017991455')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal('2.718281828459045235360287471').ln()) == (
    "Decimal('0.9999999999999999999999999999')",
    ['Inexact', 'Rounded'],
)
assert run(lambda: Decimal('1.0000001').ln()) == ("Decimal('9.999999500000033333330833334E-8')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal(100).log10()) == ("Decimal('2')", [])
assert run(lambda: Decimal('0.001').log10()) == ("Decimal('-3')", [])
assert run(lambda: Decimal(2).log10()) == ("Decimal('0.3010299956639811952137388947')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal(1).log10()) == ("Decimal('0')", [])
assert run(lambda: Decimal(0).log10()) == ("Decimal('-Infinity')", [])
assert run(lambda: Decimal(-5).log10()) == ("![<class 'decimal.InvalidOperation'>]", ['InvalidOperation'])
assert run(lambda: Decimal('inf').log10()) == ("Decimal('Infinity')", [])
assert run(lambda: Decimal('1E+999999').log10()) == ("Decimal('999999')", [])
assert run(lambda: Decimal('1000.0').log10()) == ("Decimal('3')", [])
assert run(lambda: Decimal('0.5').log10()) == ("Decimal('-0.3010299956639811952137388947')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal('9.99').log10()) == ("Decimal('0.9995654882259823086935343993')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal(2).ln().exp()) == ("Decimal('2.000000000000000000000000000')", ['Inexact', 'Rounded'])
assert run(lambda: Decimal('12345.6789').sqrt()) == ("Decimal('111.1111106055555544054166614')", ['Inexact', 'Rounded'])

# === prec=5, ROUND_HALF_EVEN: pow respects the rounding, the others round half-even ===
with localcontext(prec=5, rounding=ROUND_HALF_EVEN):
    assert run(lambda: Decimal(10) ** 99) == ("Decimal('1.0000E+99')", ['Rounded'])
    assert run(lambda: Decimal(10) ** 100) == ("Decimal('1.0000E+100')", ['Rounded'])
    assert run(lambda: Decimal(10) ** -105) == ("Decimal('1E-105')", [])
    assert run(lambda: Decimal(10) ** -110) == ("Decimal('1E-110')", [])
    assert run(lambda: Decimal(2) ** -340) == ("Decimal('4.4648E-103')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(300).exp()) == ("Decimal('1.9424E+130')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(-300).exp()) == ("Decimal('5.1482E-131')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('1e-99').sqrt()) == ("Decimal('3.1623E-50')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('0.1') ** Decimal('100.5')) == ("Decimal('3.1623E-101')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('4') ** Decimal('-0.5')) == ("Decimal('0.50000')", ['Inexact', 'Rounded'])

# === prec=50, ROUND_HALF_EVEN: pow respects the rounding, the others round half-even ===
with localcontext(prec=50, rounding=ROUND_HALF_EVEN):
    assert run(lambda: Decimal(10) ** 99) == (
        "Decimal('1.0000000000000000000000000000000000000000000000000E+99')",
        ['Rounded'],
    )
    assert run(lambda: Decimal(10) ** 100) == (
        "Decimal('1.0000000000000000000000000000000000000000000000000E+100')",
        ['Rounded'],
    )
    assert run(lambda: Decimal(10) ** -105) == ("Decimal('1E-105')", [])
    assert run(lambda: Decimal(10) ** -110) == ("Decimal('1E-110')", [])
    assert run(lambda: Decimal(2) ** -340) == (
        "Decimal('4.4647944971963866492804044855677381228590234172766E-103')",
        ['Inexact', 'Rounded'],
    )
    assert run(lambda: Decimal(300).exp()) == (
        "Decimal('1.9424263952412559365842088360176992193662086219516E+130')",
        ['Inexact', 'Rounded'],
    )
    assert run(lambda: Decimal(-300).exp()) == (
        "Decimal('5.1482002224120137811548619210671309981349982244444E-131')",
        ['Inexact', 'Rounded'],
    )
    assert run(lambda: Decimal('1e-99').sqrt()) == (
        "Decimal('3.1622776601683793319988935444327185337195551393252E-50')",
        ['Inexact', 'Rounded'],
    )
    assert run(lambda: Decimal('0.1') ** Decimal('100.5')) == (
        "Decimal('3.1622776601683793319988935444327185337195551393252E-101')",
        ['Inexact', 'Rounded'],
    )
    assert run(lambda: Decimal('4') ** Decimal('-0.5')) == (
        "Decimal('0.50000000000000000000000000000000000000000000000000')",
        ['Inexact', 'Rounded'],
    )

# === prec=9, ROUND_UP: pow respects the rounding, the others round half-even ===
with localcontext(prec=9, rounding=ROUND_UP):
    assert run(lambda: Decimal(10) ** 99) == ("Decimal('1.00000000E+99')", ['Rounded'])
    assert run(lambda: Decimal(10) ** 100) == ("Decimal('1.00000000E+100')", ['Rounded'])
    assert run(lambda: Decimal(10) ** -105) == ("Decimal('1E-105')", [])
    assert run(lambda: Decimal(10) ** -110) == ("Decimal('1E-110')", [])
    assert run(lambda: Decimal(2) ** -340) == ("Decimal('4.46479450E-103')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(300).exp()) == ("Decimal('1.94242640E+130')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(-300).exp()) == ("Decimal('5.14820022E-131')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('1e-99').sqrt()) == ("Decimal('3.16227766E-50')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('0.1') ** Decimal('100.5')) == ("Decimal('3.16227767E-101')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('4') ** Decimal('-0.5')) == ("Decimal('0.500000000')", ['Inexact', 'Rounded'])

# === prec=9, ROUND_DOWN: pow respects the rounding, the others round half-even ===
with localcontext(prec=9, rounding=ROUND_DOWN):
    assert run(lambda: Decimal(10) ** 99) == ("Decimal('1.00000000E+99')", ['Rounded'])
    assert run(lambda: Decimal(10) ** 100) == ("Decimal('1.00000000E+100')", ['Rounded'])
    assert run(lambda: Decimal(10) ** -105) == ("Decimal('1E-105')", [])
    assert run(lambda: Decimal(10) ** -110) == ("Decimal('1E-110')", [])
    assert run(lambda: Decimal(2) ** -340) == ("Decimal('4.46479449E-103')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(300).exp()) == ("Decimal('1.94242640E+130')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(-300).exp()) == ("Decimal('5.14820022E-131')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('1e-99').sqrt()) == ("Decimal('3.16227766E-50')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('0.1') ** Decimal('100.5')) == ("Decimal('3.16227766E-101')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('4') ** Decimal('-0.5')) == ("Decimal('0.499999999')", ['Inexact', 'Rounded'])

# === prec=9, ROUND_FLOOR: pow respects the rounding, the others round half-even ===
with localcontext(prec=9, rounding=ROUND_FLOOR):
    assert run(lambda: Decimal(10) ** 99) == ("Decimal('1.00000000E+99')", ['Rounded'])
    assert run(lambda: Decimal(10) ** 100) == ("Decimal('1.00000000E+100')", ['Rounded'])
    assert run(lambda: Decimal(10) ** -105) == ("Decimal('1E-105')", [])
    assert run(lambda: Decimal(10) ** -110) == ("Decimal('1E-110')", [])
    assert run(lambda: Decimal(2) ** -340) == ("Decimal('4.46479449E-103')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(300).exp()) == ("Decimal('1.94242640E+130')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(-300).exp()) == ("Decimal('5.14820022E-131')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('1e-99').sqrt()) == ("Decimal('3.16227766E-50')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('0.1') ** Decimal('100.5')) == ("Decimal('3.16227766E-101')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('4') ** Decimal('-0.5')) == ("Decimal('0.499999999')", ['Inexact', 'Rounded'])

# === prec=9, ROUND_CEILING: pow respects the rounding, the others round half-even ===
with localcontext(prec=9, rounding=ROUND_CEILING):
    assert run(lambda: Decimal(10) ** 99) == ("Decimal('1.00000000E+99')", ['Rounded'])
    assert run(lambda: Decimal(10) ** 100) == ("Decimal('1.00000000E+100')", ['Rounded'])
    assert run(lambda: Decimal(10) ** -105) == ("Decimal('1E-105')", [])
    assert run(lambda: Decimal(10) ** -110) == ("Decimal('1E-110')", [])
    assert run(lambda: Decimal(2) ** -340) == ("Decimal('4.46479450E-103')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(300).exp()) == ("Decimal('1.94242640E+130')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(-300).exp()) == ("Decimal('5.14820022E-131')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('1e-99').sqrt()) == ("Decimal('3.16227766E-50')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('0.1') ** Decimal('100.5')) == ("Decimal('3.16227767E-101')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('4') ** Decimal('-0.5')) == ("Decimal('0.500000000')", ['Inexact', 'Rounded'])

# === Overflow, underflow and subnormal results ===
with localcontext(Emax=99, Emin=-99, prec=9):
    assert run(lambda: Decimal(10) ** 95) == ("Decimal('1.00000000E+95')", ['Rounded'])
    assert run(lambda: Decimal('1e50').sqrt()) == ("Decimal('1E+25')", [])

# === Clamping ===
with localcontext(clamp=1, Emax=99, Emin=-99, prec=9):
    assert run(lambda: Decimal(2) ** Decimal('0.5')) == ("Decimal('1.41421356')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(2) ** 100) == ("Decimal('1.26765060E+30')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(-3) ** 41) == ("Decimal('-3.64729964E+19')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(2).sqrt()) == ("Decimal('1.41421356')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(3).exp()) == ("Decimal('20.0855369')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(-3).exp()) == ("Decimal('0.0497870684')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(7).ln()) == ("Decimal('1.94591015')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(7).log10()) == ("Decimal('0.845098040')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal('0.3').ln()) == ("Decimal('-1.20397280')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(2) ** -100) == ("Decimal('7.88860905E-31')", ['Inexact', 'Rounded'])
    assert run(lambda: Decimal(123456) ** 2) == ("Decimal('1.52413839E+10')", ['Inexact', 'Rounded'])
    assert run(lambda: pow(Decimal(3), 200, 99999)) == ("Decimal('9513')", [])

# === Trapped signals raise the specific class ===
try:
    Decimal(0) ** 0
    assert False, 'expected InvalidOperation'
except InvalidOperation as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"
try:
    Decimal(10) ** Decimal(10**7)
    assert False, 'expected Overflow'
except Overflow as e:
    assert str(e) == "[<class 'decimal.Overflow'>]"
try:
    Decimal(-1).ln()
    assert False, 'expected InvalidOperation'
except ArithmeticError as e:
    assert str(e) == "[<class 'decimal.InvalidOperation'>]"

# === Untrapped InvalidOperation yields NaN ===
with localcontext(traps=[]):
    assert repr(Decimal(-1).sqrt()) == "Decimal('NaN')"
    assert repr(Decimal(0) ** 0) == "Decimal('NaN')"
    assert repr(pow(Decimal(2), 3, 0)) == "Decimal('NaN')"
    assert repr(Decimal('-sNaN7').exp()) == "Decimal('-NaN7')"

# === Context methods use their own context ===
c = Context(prec=6)
assert repr(c.power(Decimal(2), Decimal('0.5'))) == "Decimal('1.41421')"
assert repr(c.power(2, 10, 1000)) == "Decimal('24')"
assert repr(c.sqrt(Decimal(3))) == "Decimal('1.73205')"
assert repr(c.exp(Decimal(1))) == "Decimal('2.71828')"
assert repr(c.ln(Decimal(2))) == "Decimal('0.693147')"
assert repr(c.log10(Decimal(5))) == "Decimal('0.698970')"
assert c.flags[Inexact]

# === Higher precision: digits of ln(10) beyond the built-in table ===
with localcontext(prec=60):
    assert repr(Decimal(10).ln()) == "Decimal('2.30258509299404568401799145468436420760110148862877297603333')"
    assert repr(Decimal(2).sqrt()) == "Decimal('1.41421356237309504880168872420969807856967187537694807317668')"
    assert repr(Decimal(1).exp()) == "Decimal('2.71828182845904523536028747135266249775724709369995957496697')"

# === The math module converts a Decimal through __float__ ===
assert math.sqrt(Decimal('2.25')) == 1.5
assert math.isclose(Decimal(1), 1)
assert math.log(Decimal(100), 10) == 2.0
assert math.log10(Decimal(1000)) == 3.0
assert math.hypot(Decimal(3), Decimal(4)) == 5.0
assert math.fsum([Decimal('0.1')] * 10) == 1.0
assert math.isnan(Decimal('NaN'))
assert math.isinf(Decimal('-Infinity'))
assert math.copysign(Decimal(1), Decimal(-2)) == -1.0
assert math.sqrt(Decimal('1e999999')) == math.inf
try:
    math.sqrt(Decimal('sNaN'))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'cannot convert signaling NaN to float'
try:
    math.log(Decimal(-1))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'expected a positive input, got -1.0'
