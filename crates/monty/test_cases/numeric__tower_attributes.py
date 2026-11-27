"""Tests for numeric tower attributes (real, imag, numerator, denominator, conjugate)."""

import math

# --- int attributes ---
for val in [0, 1, 42, -1, -5, -100, 10**30, -(10**30), 1 << 100, -(1 << 100)]:
    assert val.real == val
    assert type(val.real) is int
    assert val.imag == 0
    assert type(val.imag) is int
    assert val.numerator == val
    assert type(val.numerator) is int
    assert val.denominator == 1
    assert type(val.denominator) is int
    assert val.conjugate() == val
    assert type(val.conjugate()) is int

# --- bool attributes (coerces to int) ---
assert True.real == 1
assert type(True.real) is int
assert True.imag == 0
assert type(True.imag) is int
assert True.numerator == 1
assert type(True.numerator) is int
assert True.denominator == 1
assert type(True.denominator) is int
assert True.conjugate() == 1
assert type(True.conjugate()) is int

assert False.real == 0
assert type(False.real) is int
assert False.imag == 0
assert type(False.imag) is int
assert False.numerator == 0
assert type(False.numerator) is int
assert False.denominator == 1
assert type(False.denominator) is int
assert False.conjugate() == 0
assert type(False.conjugate()) is int

# --- float attributes ---
for f in [0.0, 3.14, -2.5, 1e20, float('inf'), float('-inf')]:
    assert f.real == f
    assert type(f.real) is float
    assert f.imag == 0.0
    assert type(f.imag) is float
    assert f.conjugate() == f
    assert type(f.conjugate()) is float

# Signed zeroes for float:
neg_zero = -0.0
assert neg_zero.real == 0.0
assert math.copysign(1.0, neg_zero.real) == -1.0
assert math.copysign(1.0, neg_zero.conjugate()) == -1.0
# Imaginary part is ALWAYS positive 0.0:
assert neg_zero.imag == 0.0
assert math.copysign(1.0, neg_zero.imag) == 1.0

# NaN float:
nan_val = float('nan')
assert math.isnan(nan_val.real)
assert nan_val.imag == 0.0
assert math.copysign(1.0, nan_val.imag) == 1.0
assert math.isnan(nan_val.conjugate())

# float does NOT have numerator / denominator
try:
    (1.5).numerator
except AttributeError as e:
    assert str(e) == "'float' object has no attribute 'numerator'"
else:
    raise AssertionError('expected AttributeError for float.numerator')

try:
    (1.5).denominator
except AttributeError as e:
    assert str(e) == "'float' object has no attribute 'denominator'"
else:
    raise AssertionError('expected AttributeError for float.denominator')

# --- conjugate() argument validation ---
try:
    (1).conjugate(1)
except TypeError as e:
    assert str(e) == 'int.conjugate() takes no arguments (1 given)'
else:
    raise AssertionError('expected TypeError')

try:
    (1).conjugate(x=1)
except TypeError as e:
    assert str(e) == 'int.conjugate() takes no keyword arguments'
else:
    raise AssertionError('expected TypeError')

try:
    True.conjugate(1)
except TypeError as e:
    assert str(e) == 'int.conjugate() takes no arguments (1 given)'
else:
    raise AssertionError('expected TypeError')

try:
    True.conjugate(x=1)
except TypeError as e:
    assert str(e) == 'int.conjugate() takes no keyword arguments'
else:
    raise AssertionError('expected TypeError')

try:
    (10**30).conjugate(1)
except TypeError as e:
    assert str(e) == 'int.conjugate() takes no arguments (1 given)'
else:
    raise AssertionError('expected TypeError')

try:
    (10**30).conjugate(x=1)
except TypeError as e:
    assert str(e) == 'int.conjugate() takes no keyword arguments'
else:
    raise AssertionError('expected TypeError')

try:
    (1.5).conjugate(1)
except TypeError as e:
    assert str(e) == 'float.conjugate() takes no arguments (1 given)'
else:
    raise AssertionError('expected TypeError')

try:
    (1.5).conjugate(x=1)
except TypeError as e:
    assert str(e) == 'float.conjugate() takes no keyword arguments'
else:
    raise AssertionError('expected TypeError')

# --- read-only assignment validation ---
for attr_name in ['real', 'imag', 'numerator', 'denominator']:
    try:
        setattr(1, attr_name, 99)
    except AttributeError as e:
        assert str(e) == f"attribute '{attr_name}' of 'int' objects is not writable"
    else:
        raise AssertionError(f'expected AttributeError for int.{attr_name} assignment')

    try:
        setattr(10**30, attr_name, 99)
    except AttributeError as e:
        assert str(e) == f"attribute '{attr_name}' of 'int' objects is not writable"
    else:
        raise AssertionError(f'expected AttributeError for large int.{attr_name} assignment')

    try:
        setattr(True, attr_name, 99)
    except AttributeError as e:
        assert str(e) == f"attribute '{attr_name}' of 'int' objects is not writable"
    else:
        raise AssertionError(f'expected AttributeError for bool.{attr_name} assignment')

for attr_name in ['real', 'imag']:
    try:
        setattr(1.5, attr_name, 99.0)
    except AttributeError as e:
        assert str(e) == f"attribute '{attr_name}' of 'float' objects is not writable"
    else:
        raise AssertionError(f'expected AttributeError for float.{attr_name} assignment')

# --- interned (literal) large int attributes and validation ---
INTERNED_LONG = 1234567890123456789012345678901234567890
assert INTERNED_LONG.real == INTERNED_LONG
assert type(INTERNED_LONG.real) is int
assert INTERNED_LONG.imag == 0
assert type(INTERNED_LONG.imag) is int
assert INTERNED_LONG.numerator == INTERNED_LONG
assert type(INTERNED_LONG.numerator) is int
assert INTERNED_LONG.denominator == 1
assert type(INTERNED_LONG.denominator) is int
assert INTERNED_LONG.conjugate() == INTERNED_LONG
assert type(INTERNED_LONG.conjugate()) is int

try:
    INTERNED_LONG.conjugate(1)
except TypeError as e:
    assert str(e) == 'int.conjugate() takes no arguments (1 given)'
else:
    raise AssertionError('expected TypeError')

for attr_name in ['real', 'imag', 'numerator', 'denominator']:
    try:
        setattr(INTERNED_LONG, attr_name, 99)
    except AttributeError as e:
        assert str(e) == f"attribute '{attr_name}' of 'int' objects is not writable"
    else:
        raise AssertionError(f'expected AttributeError for interned int.{attr_name} assignment')

# --- unknown attributes and setattr on non-existing attributes ---
for val in [1, INTERNED_LONG, 10**30, True, 1.5]:
    try:
        val.nonexistent_attribute
    except AttributeError:
        pass
    else:
        raise AssertionError('expected AttributeError')

    assert not hasattr(val, 'nonexistent_attribute')
    assert getattr(val, 'nonexistent_attribute', 42) == 42

    try:
        setattr(val, 'nonexistent_attribute', 99)
    except AttributeError as e:
        expected_type = 'bool' if type(val) is bool else ('float' if type(val) is float else 'int')
        assert str(e) == f"'{expected_type}' object has no attribute 'nonexistent_attribute' and no __dict__ for setting new attributes"
    else:
        raise AssertionError('expected AttributeError for setattr nonexistent')

