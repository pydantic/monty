# === i64::MIN // -1 overflow ===
INT_MIN = -(2**63)
INT_MAX = 2**63 - 1

assert INT_MIN // -1 == 9223372036854775808, 'INT_MIN // -1'
assert INT_MIN % -1 == 0, 'INT_MIN % -1'

q, r = divmod(INT_MIN, -1)
assert q == 9223372036854775808, 'divmod(INT_MIN, -1) quot'
assert r == 0, 'divmod(INT_MIN, -1) rem'

# === augmented assignment ===
x = INT_MIN
x //= -1
assert x == 9223372036854775808, 'INT_MIN //= -1'

x = INT_MIN
x %= -1
assert x == 0, 'INT_MIN %= -1'

# === i64 boundary values ===
assert INT_MIN // 1 == INT_MIN, 'INT_MIN // 1'
assert INT_MIN // -2 == 4611686018427387904, 'INT_MIN // -2'
assert INT_MIN % 1 == 0, 'INT_MIN % 1'
assert INT_MIN % -2 == 0, 'INT_MIN % -2'
assert INT_MIN % 3 == 1, 'INT_MIN % 3'

assert INT_MAX // -1 == -INT_MAX, 'INT_MAX // -1'
assert INT_MAX % -1 == 0, 'INT_MAX % -1'
assert INT_MAX // 2 == 4611686018427387903, 'INT_MAX // 2'
assert INT_MAX % 2 == 1, 'INT_MAX % 2'

# === boundary divisors ===
assert INT_MIN // INT_MIN == 1, 'INT_MIN // INT_MIN'
assert INT_MIN // INT_MAX == -2, 'INT_MIN // INT_MAX'
assert INT_MAX // INT_MIN == -1, 'INT_MAX // INT_MIN'
assert INT_MIN % INT_MIN == 0, 'INT_MIN % INT_MIN'
assert INT_MAX % INT_MAX == 0, 'INT_MAX % INT_MAX'

# === sign combinations ===
assert -7 // 2 == -4, '-7 // 2'
assert 7 // -2 == -4, '7 // -2'
assert -7 % 2 == 1, '-7 % 2'
assert 7 % -2 == -1, '7 % -2'

q, r = divmod(-7, 2)
assert q == -4, 'divmod(-7, 2) quot'
assert r == 1, 'divmod(-7, 2) rem'

q, r = divmod(7, -2)
assert q == -4, 'divmod(7, -2) quot'
assert r == -1, 'divmod(7, -2) rem'

# === divmod at boundaries ===
q, r = divmod(INT_MIN, 2)
assert q == -4611686018427387904, 'divmod(INT_MIN, 2) quot'
assert r == 0, 'divmod(INT_MIN, 2) rem'

q, r = divmod(INT_MAX, -1)
assert q == -INT_MAX, 'divmod(INT_MAX, -1) quot'
assert r == 0, 'divmod(INT_MAX, -1) rem'

q, r = divmod(INT_MIN, INT_MAX)
assert q == -2, 'divmod(INT_MIN, INT_MAX) quot'
assert r == INT_MAX - 1, 'divmod(INT_MIN, INT_MAX) rem'

# === divmod invariant: q * b + r == a ===
q, r = divmod(INT_MIN, -1)
assert q * -1 + r == INT_MIN, 'divmod(INT_MIN, -1) invariant'

q, r = divmod(INT_MIN, 3)
assert q * 3 + r == INT_MIN, 'divmod(INT_MIN, 3) invariant'
assert q == -3074457345618258603, 'divmod(INT_MIN, 3) quot'
assert r == 1, 'divmod(INT_MIN, 3) rem'

# === CompareModEq patterns ===
x = INT_MIN
assert x % -1 == 0, 'INT_MIN % -1 == 0'
assert x % 2 == 0, 'INT_MIN % 2 == 0'
assert x % 3 == 1, 'INT_MIN % 3 == 1'

x = INT_MAX
assert x % -1 == 0, 'INT_MAX % -1 == 0'
assert x % 2 == 1, 'INT_MAX % 2 == 1'
