from decimal import Decimal as D, InvalidOperation

# === Builtins: abs / int / float ===
assert str(abs(D('-3.5'))) == '3.5', 'abs negative'
assert str(abs(D('3.5'))) == '3.5', 'abs positive'
assert str(abs(D('-inf'))) == 'Infinity', 'abs -inf'
assert int(D('1.7')) == 1, 'int truncates'
assert int(D('-1.7')) == -1, 'int truncates toward zero'
assert int(D('5')) == 5, 'int exact'
assert int(D('1E+2')) == 100, 'int from exponent form'
assert float(D('1.5')) == 1.5, 'float'
assert float(D('inf')) == float('inf'), 'float inf'

# === round ===
assert round(D('2.5')) == 2, 'round half-even down'
assert round(D('3.5')) == 4, 'round half-even up'
assert round(D('2.5')) == 2 and type(round(D('2.5'))) is int, 'round no-arg returns int'
assert str(round(D('2.675'), 2)) == '2.68', 'round to 2 places'
assert str(round(D('2.5'), 0)) == '2', 'round to 0 places returns Decimal'
assert str(round(D('123.456'), -1)) == '1.2E+2', 'round negative ndigits'
assert type(round(D('2.5'), 1)) is D, 'round with ndigits returns Decimal'
# round of specials / over-precision (CPython __round__ semantics)
assert str(round(D('nan'), 2)) == 'NaN', 'round(NaN, n) stays NaN'
try:
    round(D('inf'), 2)
    assert False, 'expected InvalidOperation'
except InvalidOperation as exc:
    assert str(exc) == "[<class 'decimal.InvalidOperation'>]", 'round(inf, n) signals InvalidOperation'
try:
    round(D('1.5'), 100)
    assert False, 'expected InvalidOperation'
except InvalidOperation as exc:
    assert str(exc) == "[<class 'decimal.InvalidOperation'>]", 'round past precision signals InvalidOperation'

# === Predicates ===
assert D('nan').is_nan() and not D('1.5').is_nan(), 'is_nan'
assert D('inf').is_infinite() and not D('1.5').is_infinite(), 'is_infinite'
assert D('1.5').is_finite() and not D('inf').is_finite() and not D('nan').is_finite(), 'is_finite'
assert D('0').is_zero() and D('-0').is_zero() and not D('1').is_zero(), 'is_zero'
assert D('-1.5').is_signed() and D('-0').is_signed() and not D('1.5').is_signed(), 'is_signed'
assert D('nan').is_qnan() and not D('nan').is_snan(), 'is_qnan / is_snan'
assert not D('1.5').is_snan(), 'is_snan always false'

# === Methods returning Decimal ===
assert str(D('9').sqrt()) == '3', 'sqrt perfect square'
assert str(D('2').sqrt()) == '1.414213562373095048801688724', 'sqrt to prec'
assert str(D('1.23456').quantize(D('0.01'))) == '1.23', 'quantize down'
assert str(D('1.5').quantize(D('1'))) == '2', 'quantize to integer (half-even)'
assert str(D('1.20').normalize()) == '1.2', 'normalize strips zeros'
assert str(D('100').normalize()) == '1E+2', 'normalize trailing integer zeros'
assert str(D('1.7').to_integral_value()) == '2', 'to_integral_value rounds'
assert str(D('-1.5').copy_abs()) == '1.5', 'copy_abs'
assert str(D('1.5').copy_negate()) == '-1.5', 'copy_negate'
assert str(D('1.5').copy_sign(D('-2'))) == '-1.5', 'copy_sign'
assert str(D('1000').log10()) == '3', 'log10'
assert str(D('1').exp()) == '2.718281828459045235360287471', 'exp'

# === ln (natural log) ===
assert str(D('1').ln()) == '0', 'ln of 1 is exactly 0'
assert str(D('nan').ln()) == 'NaN', 'ln of NaN propagates quietly'
try:
    D('-1').ln()
    assert False, 'expected InvalidOperation'
except InvalidOperation as exc:
    assert str(exc) == "[<class 'decimal.InvalidOperation'>]", 'ln of a negative signals InvalidOperation'

# === quantize: infinities and precision overflow ===
assert str(D('inf').quantize(D('inf'))) == 'Infinity', 'inf.quantize(inf) keeps infinity'
assert str(D('-inf').quantize(D('inf'))) == '-Infinity', 'inf.quantize(inf) keeps sign'
try:
    D('inf').quantize(D('1'))
    assert False, 'expected InvalidOperation'
except InvalidOperation as exc:
    assert str(exc) == "[<class 'decimal.InvalidOperation'>]", 'finite/infinite quantize is invalid'
try:
    D('1e100').quantize(D('1e-100'))
    assert False, 'expected InvalidOperation'
except InvalidOperation as exc:
    assert str(exc) == "[<class 'decimal.InvalidOperation'>]", 'quantize past precision is invalid'

# === adjusted ===
assert D('1.234').adjusted() == 0, 'adjusted fractional'
assert D('1E+5').adjusted() == 5, 'adjusted exponent'
assert D('100').adjusted() == 2, 'adjusted integer'
# A zero coefficient counts as one digit, so adjusted == exponent for any zero.
assert D('0').adjusted() == 0, 'adjusted zero'
assert D('0.00').adjusted() == -2, 'adjusted zero with scale'
assert D('0E+5').adjusted() == 5, 'adjusted zero with positive exponent'

# === as_tuple (DecimalTuple namedtuple) ===
t = D('1.20').as_tuple()
assert (t.sign, t.digits, t.exponent) == (0, (1, 2, 0), -2), 'as_tuple fields'
assert t[0] == 0 and t[1] == (1, 2, 0) and t[2] == -2, 'as_tuple indexing'
neg = D('-0.5').as_tuple()
assert (neg.sign, neg.digits, neg.exponent) == (1, (5,), -1), 'as_tuple negative'
inf_t = D('inf').as_tuple()
assert (inf_t.sign, inf_t.digits, inf_t.exponent) == (0, (0,), 'F'), 'as_tuple infinity'
# repr uses the CPython class name `DecimalTuple`, not the snake_case default.
assert repr(t) == 'DecimalTuple(sign=0, digits=(1, 2, 0), exponent=-2)', 'as_tuple repr class name'

# === Tuple/list constructor ===
assert str(D((0, (1, 2, 0), -2))) == '1.20', 'tuple constructor'
assert str(D((1, (5,), 0))) == '-5', 'tuple constructor negative'
assert str(D((0, (), 'F'))) == 'Infinity', 'tuple constructor infinity'
assert str(D((0, (3, 1, 4), -2))) == '3.14', 'tuple constructor decimal'
# `bool` is accepted everywhere an `int` is (sign, digits, exponent).
assert str(D((False, (1,), 0))) == '1', 'tuple constructor bool sign'
assert str(D((0, (True,), 0))) == '1', 'tuple constructor bool digit'
assert str(D((0, (1,), True))) == '1E+1', 'tuple constructor bool exponent'

# round-trip: as_tuple -> Decimal
original = D('-12.34')
assert D(original.as_tuple()) == original, 'as_tuple round-trips through constructor'

# === Tuple constructor errors ===
try:
    D([1, 2])
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'argument must be a sequence of length 3', 'wrong length message'

try:
    D((2, (1,), 0))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'sign must be an integer with the value 0 or 1', 'bad sign message'

try:
    D((0, 5, 0))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'coefficient must be a tuple of digits', 'non-sequence coefficient message'

try:
    D((0, (10,), 0))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'coefficient must be a tuple of digits', 'out-of-range digit message'

try:
    D((0, (1,), 1.5))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'exponent must be an integer', 'float exponent message'

try:
    D((0, (1,), 'd'))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == "string argument in the third position must be 'F', 'n' or 'N'", 'bad marker message'

# === int/float errors on specials ===
try:
    int(D('inf'))
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == 'cannot convert Infinity to integer', 'int inf message'

try:
    int(D('nan'))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'cannot convert NaN to integer', 'int nan message'

# === Method arity: a spurious argument raises (no refcount leak) ===
# `as_tuple` / `copy_abs` allocate a heap result; the argument count must be
# checked *before* computing, or the result leaks (and panics under
# memory-model-checks). The error is qualified `Decimal.<method>()` like CPython.
try:
    D('1.20').as_tuple(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Decimal.as_tuple() takes no arguments (1 given)', 'as_tuple arity'

try:
    D('-1.5').copy_abs(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Decimal.copy_abs() takes no arguments (1 given)', 'copy_abs arity'

try:
    D('1').is_nan(5)
    assert False, 'expected TypeError'
except TypeError as exc:
    assert str(exc) == 'Decimal.is_nan() takes no arguments (1 given)', 'predicate arity'

# === Per-call rounding= on quantize (all eight modes) ===
import decimal

cents = D('0.01')
for mode, expected in [
    (decimal.ROUND_CEILING, '7.33'),
    (decimal.ROUND_FLOOR, '7.32'),
    (decimal.ROUND_DOWN, '7.32'),
    (decimal.ROUND_UP, '7.33'),
    (decimal.ROUND_HALF_UP, '7.33'),
    (decimal.ROUND_HALF_DOWN, '7.32'),
    (decimal.ROUND_HALF_EVEN, '7.32'),
    (decimal.ROUND_05UP, '7.32'),
]:
    assert str(D('7.325').quantize(cents, rounding=mode)) == expected, f'quantize {mode}'
assert str(D('-7.325').quantize(cents, rounding=decimal.ROUND_CEILING)) == '-7.32', 'ceiling toward +inf'
assert str(D('-7.325').quantize(cents, rounding=decimal.ROUND_FLOOR)) == '-7.33', 'floor toward -inf'
assert str(D('7.505').quantize(cents, rounding=decimal.ROUND_05UP)) == '7.51', '05up on kept 0'
assert str(D('7.325').quantize(cents)) == '7.32', 'quantize default HALF_EVEN'
assert str(D('7.325').quantize(cents, None)) == '7.32', 'explicit rounding=None'

# rounding= also works positionally and on to_integral_value
assert str(D('7.325').quantize(cents, decimal.ROUND_UP)) == '7.33', 'positional rounding'
assert str(D('7.7').to_integral_value(rounding=decimal.ROUND_FLOOR)) == '7', 'to_integral_value rounding'
assert str(D('-7.1').to_integral_value(rounding=decimal.ROUND_CEILING)) == '-7', 'to_integral ceiling'

# An invalid rounding value (bad string or non-string) raises CPython's TypeError.
for bad in ['ROUND_SIDEWAYS', 3]:
    try:
        D('1').quantize(cents, rounding=bad)
        assert False, 'expected TypeError for bad rounding'
    except TypeError as exc:
        assert str(exc) == (
            'valid values for rounding are:\n  [ROUND_CEILING, ROUND_FLOOR, ROUND_UP, ROUND_DOWN,\n'
            '   ROUND_HALF_UP, ROUND_HALF_DOWN, ROUND_HALF_EVEN,\n   ROUND_05UP]'
        ), f'bad rounding message: {exc}'

# === to_integral_value keeps positive-exponent values unchanged ===
assert str(D('1E+30').to_integral_value()) == '1E+30', 'positive exponent unchanged'
assert str(D('1E+300').to_integral_value()) == '1E+300', 'huge positive exponent unchanged'

# === sqrt at extreme magnitudes ===
assert str(D('1E+1000').sqrt()) == '1E+500', 'sqrt of huge magnitude'
assert str(D('1E-1000').sqrt()) == '1E-500', 'sqrt of tiny magnitude'
assert str(D(2).sqrt()) == '1.414213562373095048801688724', 'sqrt(2) correctly rounded'

# === Transcendentals: exact CPython digits ===
assert str(D(2).ln()) == '0.6931471805599453094172321215', 'ln(2)'
assert str(D(3).log10()) == '0.4771212547196624372950279033', 'log10(3)'
assert str(D(1).exp()) == '2.718281828459045235360287471', 'exp(1)'
assert str(D(-40).exp()) == '4.248354255291588995329234783E-18', 'exp(-40)'
assert str(D(0).ln()) == '-Infinity', 'ln(0)'
assert str(D('-0').ln()) == '-Infinity', 'ln(-0)'
assert str(D(0).log10()) == '-Infinity', 'log10(0)'

# === exp() overflow raises (never saturates or aborts) ===
for operand in ['1E+30', '1E16000']:
    try:
        D(operand).exp()
        assert False, 'expected Overflow from exp'
    except decimal.Overflow as exc:
        assert str(exc) == "[<class 'decimal.Overflow'>]", f'exp({operand}) message'

# === Signaling NaN through conversions ===
try:
    float(D('sNaN'))
    assert False, 'expected ValueError from float(sNaN)'
except ValueError as exc:
    assert str(exc) == 'cannot convert signaling NaN to float', 'float sNaN message'
try:
    int(D('sNaN'))
    assert False, 'expected ValueError from int(sNaN)'
except ValueError as exc:
    assert str(exc) == 'cannot convert NaN to integer', 'int sNaN message'
