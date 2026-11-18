# `float(str)` and `float(bytes)`: the forms CPython's parser accepts and the ones it rejects.

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


# === Bytes: interned literals and dynamically allocated input ===
def heap_bytes(value):
    return (b'!' + value)[1:]


for raw, expected in (
    (b'1.5', 1.5),
    (b' \t\n-2.25\r\v\f', -2.25),
    (b'.5', 0.5),
    (b'5.', 5.0),
    (b'+1e3', 1000.0),
    (b'1_000.5', 1000.5),
    (b'1e1_0', 1e10),
    (b'+Infinity', float('inf')),
    (b'-INF', -float('inf')),
):
    assert float(raw) == expected
    assert float(heap_bytes(raw)) == expected
for raw in (b'nan', b'+NaN', b'-NAN'):
    assert float(raw) != float(raw)
    assert float(heap_bytes(raw)) != float(heap_bytes(raw))
assert str(float(b'-0')) == '-0.0'
assert str(float(heap_bytes(b'-0'))) == '-0.0'

# Byte inputs must use ASCII grammar and repr the original bytes on failure.
for raw in (
    b'',
    b'   ',
    b'abc',
    b'1,5',
    b'0x10',
    b'1 2',
    b'_1',
    b'1_',
    b'1__0',
    b'1_.5',
    b'1._5',
    b'1e_5',
    b'in_f',
    b'1\x00',
    b'\x1c1.5',
    b'\xc2\xa01.5',
    b'1.5\xc2\xa0',
    b'\xd9\xa1.5',
    b'\xff',
    b'bad\'"\\\n',
):
    for value in (raw, heap_bytes(raw)):
        try:
            float(value)
            assert False, 'expected ValueError'
        except ValueError as exc:
            assert str(exc) == f'could not convert string to float: {value!r}'
