from decimal import Decimal as D
import math

# === math.floor / ceil / trunc use Decimal's __floor__/__ceil__/__trunc__ ===
assert math.floor(D('1.7')) == 1, 'floor'
assert math.floor(D('-1.7')) == -2, 'floor negative'
assert math.ceil(D('1.7')) == 2, 'ceil'
assert math.ceil(D('-1.7')) == -1, 'ceil negative'
assert math.trunc(D('1.7')) == 1, 'trunc'
assert math.trunc(D('-1.7')) == -1, 'trunc toward zero'
assert math.floor(D('2')) == 2, 'floor integral'

# === float-consuming math functions accept Decimal via __float__ ===
assert math.sqrt(D(4)) == 2.0, 'math.sqrt'
assert math.isnan(D('nan')), 'math.isnan'
assert not math.isnan(D('1.5')), 'math.isnan finite'
assert math.isinf(D('-inf')), 'math.isinf'
assert math.isfinite(D('1.5')), 'math.isfinite'
assert math.log10(D(100)) == 2.0, 'math.log10'

# === NaN / Infinity conversion errors match int() ===
try:
    math.floor(D('nan'))
    assert False, 'expected ValueError'
except ValueError as exc:
    assert str(exc) == 'cannot convert NaN to integer', 'floor NaN message'
try:
    math.ceil(D('inf'))
    assert False, 'expected OverflowError'
except OverflowError as exc:
    assert str(exc) == 'cannot convert Infinity to integer', 'ceil inf message'
