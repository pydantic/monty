# === Basic list slicing ===
lst = [0, 1, 2, 3, 4, 5]
assert lst[1:4] == [1, 2, 3], 'basic list slice'
assert lst[:3] == [0, 1, 2], 'list slice from start'
assert lst[3:] == [3, 4, 5], 'list slice to end'
assert lst[:] == [0, 1, 2, 3, 4, 5], 'list full slice'

# === Negative indices ===
assert lst[-3:] == [3, 4, 5], 'list slice negative start'
assert lst[:-2] == [0, 1, 2, 3], 'list slice negative stop'
assert lst[-4:-1] == [2, 3, 4], 'list slice both negative'

# === Step ===
assert lst[::2] == [0, 2, 4], 'list slice with step'
assert lst[1::2] == [1, 3, 5], 'list slice with start and step'
assert lst[::-1] == [5, 4, 3, 2, 1, 0], 'list reverse slice'
assert lst[4:1:-1] == [4, 3, 2], 'list negative step with bounds'
assert lst[::3] == [0, 3], 'list slice step of 3'

# === Out of bounds (clamped) ===
assert lst[10:20] == [], 'list out of bounds high'
assert lst[-100:2] == [0, 1], 'list out of bounds low'
assert lst[2:100] == [2, 3, 4, 5], 'list stop beyond length'

# === Empty results ===
assert lst[3:1] == [], 'list empty slice start > stop'
assert lst[3:3] == [], 'list empty slice start == stop'

# === String slicing ===
s = 'hello'
assert s[1:4] == 'ell', 'string slice basic'
assert s[:3] == 'hel', 'string slice from start'
assert s[3:] == 'lo', 'string slice to end'
assert s[:] == 'hello', 'string full slice'
assert s[::-1] == 'olleh', 'string reverse'
assert s[::2] == 'hlo', 'string slice with step'

# === Unicode string slicing ===
u = 'cafe'
assert u[1:3] == 'af', 'unicode slice basic'
assert u[::-1] == 'efac', 'unicode reverse'

# === Tuple slicing ===
t = (0, 1, 2, 3, 4)
assert t[1:4] == (1, 2, 3), 'tuple slice basic'
assert t[::-1] == (4, 3, 2, 1, 0), 'tuple reverse'
assert t[::2] == (0, 2, 4), 'tuple slice with step'

# === Bytes slicing ===
b = b'\x00\x01\x02\x03\x04'
assert b[1:4] == b'\x01\x02\x03', 'bytes slice basic'
assert b[::-1] == b'\x04\x03\x02\x01\x00', 'bytes reverse'
assert b[::2] == b'\x00\x02\x04', 'bytes slice with step'

# === Range slicing ===
r = range(10)
assert r[2:5] == range(2, 5), 'range slice basic'
assert r[::2] == range(0, 10, 2), 'range slice with step'

r2 = range(0, 10, 2)
assert r2[1:4] == range(2, 8, 2), 'stepped range slice'

# === slice() builtin ===
s1 = slice(3)
assert s1.start is None, 'slice stop only - start is None'
assert s1.stop == 3, 'slice stop only - stop is 3'
assert s1.step is None, 'slice stop only - step is None'

s2 = slice(1, 4)
assert s2.start == 1, 'slice start stop - start is 1'
assert s2.stop == 4, 'slice start stop - stop is 4'
assert s2.step is None, 'slice start stop - step is None'

s3 = slice(1, 10, 2)
assert s3.start == 1, 'slice full - start is 1'
assert s3.stop == 10, 'slice full - stop is 10'
assert s3.step == 2, 'slice full - step is 2'

# === Using slice objects ===
sl = slice(1, 4)
assert lst[sl] == [1, 2, 3], 'slice object for list'
assert s[sl] == 'ell', 'slice object for string'
assert t[sl] == (1, 2, 3), 'slice object for tuple'

# === slice repr and str ===
assert repr(slice(3)) == 'slice(None, 3, None)', 'slice repr stop only'
assert repr(slice(1, 4)) == 'slice(1, 4, None)', 'slice repr start stop'
assert repr(slice(1, 10, 2)) == 'slice(1, 10, 2)', 'slice repr full'
assert str(slice(1, 4)) == 'slice(1, 4, None)', 'slice str same as repr'

# === Edge case: negative step with None bounds ===
assert lst[::-2] == [5, 3, 1], 'list negative step no bounds'
assert s[::-2] == 'olh', 'string negative step no bounds'

# === Edge case: step larger than length ===
assert lst[::10] == [0], 'step larger than length'

# === Empty sequence slicing ===
empty_list = []
assert empty_list[:] == [], 'empty list full slice'
assert empty_list[1:4] == [], 'empty list any slice'
assert empty_list[::-1] == [], 'empty list reverse'

empty_str = ''
assert empty_str[:] == '', 'empty string full slice'
assert empty_str[1:4] == '', 'empty string any slice'

# === Boolean truthiness of slice ===
assert slice(1, 2), 'slice is truthy'
assert slice(None), 'slice with None stop is truthy'

# === Slice equality ===
assert slice(1, 2) == slice(1, 2), 'slice equality same values'
assert not (slice(1, 2) == slice(1, 3)), 'slice inequality different stop'
assert slice(None) == slice(None), 'slice equality both None'
assert slice(1, 2, 3) == slice(1, 2, 3), 'slice equality with step'
assert not (slice(1, 2, 3) == slice(1, 2, 4)), 'slice inequality different step'

# === Slice with bool indices ===
assert [0, 1, 2, 3][True:] == [1, 2, 3], 'slice with True start'
assert [0, 1, 2, 3][:True] == [0], 'slice with True stop'
assert [0, 1, 2, 3][::True] == [0, 1, 2, 3], 'slice with True step'
assert [0, 1, 2, 3][False:] == [0, 1, 2, 3], 'slice with False start'
assert [0, 1, 2, 3][:False] == [], 'slice with False stop'

# === Range slicing edge cases ===
assert range(0)[1:2] == range(0, 0), 'empty range slicing'
assert range(5)[::-1] == range(4, -1, -1), 'range reverse slice'
assert list(range(5)[::-1]) == [4, 3, 2, 1, 0], 'range reverse slice iteration'

# === Negative step with out-of-bounds start ===
lst5 = [0, 1, 2, 3, 4]
assert lst5[-10::-1] == [], 'far negative start with negative step should be empty'
assert lst5[-6::-1] == [], 'just out of bounds negative start'
assert lst5[-5::-1] == [0], 'exactly at first element'
assert lst5[-4::-1] == [1, 0], 'second element backwards'

# Range slicing with out-of-bounds negative start
assert list(range(5)[-10::-1]) == [], 'range far negative start'
assert list(range(5)[-6::-1]) == [], 'range just out of bounds'
assert list(range(5)[-5::-1]) == [0], 'range exactly at first'

# String slicing with out-of-bounds negative start
assert 'hello'[-10::-1] == '', 'string far negative start empty'
assert 'hello'[-5::-1] == 'h', 'string exactly at first'

# Tuple slicing with out-of-bounds negative start
assert (0, 1, 2, 3, 4)[-10::-1] == (), 'tuple far negative start empty'
assert (0, 1, 2, 3, 4)[-5::-1] == (0,), 'tuple exactly at first'
