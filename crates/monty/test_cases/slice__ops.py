# === Basic list slicing ===
lst = [0, 1, 2, 3, 4, 5]
assert lst[1:4] == [1, 2, 3]
assert lst[:3] == [0, 1, 2]
assert lst[3:] == [3, 4, 5]
assert lst[:] == [0, 1, 2, 3, 4, 5]

# === Negative indices ===
assert lst[-3:] == [3, 4, 5]
assert lst[:-2] == [0, 1, 2, 3]
assert lst[-4:-1] == [2, 3, 4]

# === Step ===
assert lst[::2] == [0, 2, 4]
assert lst[1::2] == [1, 3, 5]
assert lst[::-1] == [5, 4, 3, 2, 1, 0]
assert lst[4:1:-1] == [4, 3, 2]
assert lst[::3] == [0, 3]

# === Out of bounds (clamped) ===
assert lst[10:20] == []
assert lst[-100:2] == [0, 1]
assert lst[2:100] == [2, 3, 4, 5]

# === Empty results ===
assert lst[3:1] == []
assert lst[3:3] == []

# === String slicing ===
s = 'hello'
assert s[1:4] == 'ell'
assert s[:3] == 'hel'
assert s[3:] == 'lo'
assert s[:] == 'hello'
assert s[::-1] == 'olleh'
assert s[::2] == 'hlo'

# === Unicode string slicing ===
u = 'cafe'
assert u[1:3] == 'af'
assert u[::-1] == 'efac'

# === Tuple slicing ===
t = (0, 1, 2, 3, 4)
assert t[1:4] == (1, 2, 3)
assert t[::-1] == (4, 3, 2, 1, 0)
assert t[::2] == (0, 2, 4)

# === Bytes slicing ===
b = b'\x00\x01\x02\x03\x04'
assert b[1:4] == b'\x01\x02\x03'
assert b[::-1] == b'\x04\x03\x02\x01\x00'
assert b[::2] == b'\x00\x02\x04'

# === Range slicing ===
r = range(10)
assert r[2:5] == range(2, 5)
assert r[::2] == range(0, 10, 2)

r2 = range(0, 10, 2)
assert r2[1:4] == range(2, 8, 2)

# === slice() builtin ===
s1 = slice(3)
assert s1.start is None
assert s1.stop == 3
assert s1.step is None

s2 = slice(1, 4)
assert s2.start == 1
assert s2.stop == 4
assert s2.step is None

s3 = slice(1, 10, 2)
assert s3.start == 1
assert s3.stop == 10
assert s3.step == 2

# === Using slice objects ===
sl = slice(1, 4)
assert lst[sl] == [1, 2, 3]
assert s[sl] == 'ell'
assert t[sl] == (1, 2, 3)

# === slice repr and str ===
assert repr(slice(3)) == 'slice(None, 3, None)'
assert repr(slice(1, 4)) == 'slice(1, 4, None)'
assert repr(slice(1, 10, 2)) == 'slice(1, 10, 2)'
assert str(slice(1, 4)) == 'slice(1, 4, None)'

# === Edge case: negative step with None bounds ===
assert lst[::-2] == [5, 3, 1]
assert s[::-2] == 'olh'

# === Edge case: step larger than length ===
assert lst[::10] == [0]

# === Empty sequence slicing ===
empty_list = []
assert empty_list[:] == []
assert empty_list[1:4] == []
assert empty_list[::-1] == []

empty_str = ''
assert empty_str[:] == ''
assert empty_str[1:4] == ''

# === Boolean truthiness of slice ===
assert slice(1, 2)
assert slice(None)

# === Slice equality ===
assert slice(1, 2) == slice(1, 2)
assert not (slice(1, 2) == slice(1, 3)), 'slice inequality different stop'
assert slice(None) == slice(None)
assert slice(1, 2, 3) == slice(1, 2, 3)
assert not (slice(1, 2, 3) == slice(1, 2, 4)), 'slice inequality different step'

# === Slice with bool indices ===
assert [0, 1, 2, 3][True:] == [1, 2, 3]
assert [0, 1, 2, 3][:True] == [0]
assert [0, 1, 2, 3][::True] == [0, 1, 2, 3]
assert [0, 1, 2, 3][False:] == [0, 1, 2, 3]
assert [0, 1, 2, 3][:False] == []

# === Range slicing edge cases ===
assert range(0)[1:2] == range(0, 0)
assert range(5)[::-1] == range(4, -1, -1)
assert list(range(5)[::-1]) == [4, 3, 2, 1, 0]

# === Negative step with out-of-bounds start ===
lst5 = [0, 1, 2, 3, 4]
assert lst5[-10::-1] == []
assert lst5[-6::-1] == []
assert lst5[-5::-1] == [0]
assert lst5[-4::-1] == [1, 0]

# Range slicing with out-of-bounds negative start
assert list(range(5)[-10::-1]) == []
assert list(range(5)[-6::-1]) == []
assert list(range(5)[-5::-1]) == [0]

# String slicing with out-of-bounds negative start
assert 'hello'[-10::-1] == ''
assert 'hello'[-5::-1] == 'h'

# Tuple slicing with out-of-bounds negative start
assert (0, 1, 2, 3, 4)[-10::-1] == ()
assert (0, 1, 2, 3, 4)[-5::-1] == (0,)

# === Negative step at i64::MIN boundary ===
# Regression: step = -(2**63) used to panic because Rust's `-step` on i64::MIN overflows.
I64_MIN = -(2**63)
assert 'hello'[::I64_MIN] == 'o'
assert b'hello'[::I64_MIN] == b'o'
assert [0, 1, 2, 3, 4][::I64_MIN] == [4]
assert (0, 1, 2, 3, 4)[::I64_MIN] == (4,)
assert list(range(10)[::I64_MIN]) == [9]
