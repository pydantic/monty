# `float(str)`: the forms CPython's parser accepts and the ones it rejects.

# === Plain numbers and whitespace ===
assert float('1.5') == 1.5
assert float('  -2.25\t') == -2.25
assert float('.5') == 0.5
assert float('5.') == 5.0
assert float('+1e3') == 1000.0
assert float('1E-2') == 0.01

# === Special values, any case ===
assert float('inf') == float('+Infinity')
assert float('-INF') == -float('inf')
assert float('nan') != float('nan')

# === Underscore digit separators, strictly between digits ===
assert float('1_000') == 1000.0
assert float('1_000.5') == 1000.5
assert float('1e1_0') == 1e10
assert float(' -1_0 ') == -10.0
for bad in ('_1', '1_', '1__0', '1_.5', '1._5', '1_e5', '1e_5', 'in_f', '1_0j'):
    try:
        float(bad)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == "could not convert string to float: '" + bad + "'"

# === Rejected strings ===
for bad in ('', '   ', 'abc', '1,5', '0x10', '1 2'):
    try:
        float(bad)
        assert False, 'expected ValueError'
    except ValueError as exc:
        assert str(exc) == "could not convert string to float: '" + bad + "'"
