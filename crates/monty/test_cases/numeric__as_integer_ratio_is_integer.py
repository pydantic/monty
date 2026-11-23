# === float.as_integer_ratio() ===
assert (0.0).as_integer_ratio() == (0, 1)
assert (-0.0).as_integer_ratio() == (0, 1)
assert (1.0).as_integer_ratio() == (1, 1)
assert (-1.0).as_integer_ratio() == (-1, 1)
assert (1.5).as_integer_ratio() == (3, 2)
assert (-1.5).as_integer_ratio() == (-3, 2)
assert (0.25).as_integer_ratio() == (1, 4)
assert (-0.125).as_integer_ratio() == (-1, 8)
assert (0.1).as_integer_ratio() == (3602879701896397, 36028797018963968)
assert (42.0).as_integer_ratio() == (42, 1)
assert (1e20).as_integer_ratio() == (100000000000000000000, 1)
assert (5e-324).as_integer_ratio() == (1, 1 << 1074)
assert (-5e-324).as_integer_ratio() == (-1, 1 << 1074)

# === int.as_integer_ratio() and bool.as_integer_ratio() ===
assert (0).as_integer_ratio() == (0, 1)
assert (1).as_integer_ratio() == (1, 1)
assert (-1).as_integer_ratio() == (-1, 1)
assert (42).as_integer_ratio() == (42, 1)
assert (-9223372036854775808).as_integer_ratio() == (-9223372036854775808, 1)
big = 10**30
assert big.as_integer_ratio() == (10**30, 1)
assert (-big).as_integer_ratio() == (-(10**30), 1)

t_ratio = True.as_integer_ratio()
f_ratio = False.as_integer_ratio()
assert t_ratio == (1, 1)
assert f_ratio == (0, 1)
assert type(t_ratio[0]) is int
assert type(f_ratio[0]) is int

# === float.is_integer() ===
assert (0.0).is_integer() is True
assert (-0.0).is_integer() is True
assert (1.0).is_integer() is True
assert (-42.0).is_integer() is True
assert (1e100).is_integer() is True
assert (1.5).is_integer() is False
assert (-0.5).is_integer() is False
assert (5e-324).is_integer() is False
assert float('inf').is_integer() is False
assert float('-inf').is_integer() is False
assert float('nan').is_integer() is False

# === int.is_integer() and bool.is_integer() ===
assert (0).is_integer() is True
assert (42).is_integer() is True
assert (-42).is_integer() is True
assert (10**30).is_integer() is True
assert (-(10**30)).is_integer() is True
assert True.is_integer() is True
assert False.is_integer() is True

# === Error cases via try/except ===
try:
    float('inf').as_integer_ratio()
    assert False, 'expected OverflowError'
except OverflowError as e:
    assert str(e) == 'cannot convert Infinity to integer ratio'

try:
    float('-inf').as_integer_ratio()
    assert False, 'expected OverflowError'
except OverflowError as e:
    assert str(e) == 'cannot convert Infinity to integer ratio'

try:
    float('nan').as_integer_ratio()
    assert False, 'expected ValueError'
except ValueError as e:
    assert str(e) == 'cannot convert NaN to integer ratio'

try:
    (1.5).as_integer_ratio(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'float.as_integer_ratio() takes no arguments (1 given)'

try:
    (1.5).as_integer_ratio(x=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'float.as_integer_ratio() takes no keyword arguments'

try:
    (1).as_integer_ratio(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'int.as_integer_ratio() takes no arguments (1 given)'

try:
    True.as_integer_ratio(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'int.as_integer_ratio() takes no arguments (1 given)'

try:
    (1.5).is_integer(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'float.is_integer() takes no arguments (1 given)'

try:
    (1).is_integer(1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'int.is_integer() takes no arguments (1 given)'

try:
    False.is_integer(x=1)
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == 'int.is_integer() takes no keyword arguments'
