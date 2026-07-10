# === i64::MIN // -1 overflow ===
INT_MIN = -(2**63)
INT_MAX = 2**63 - 1

assert INT_MIN // -1 == 9223372036854775808
assert INT_MIN % -1 == 0

q, r = divmod(INT_MIN, -1)
assert q == 9223372036854775808
assert r == 0

# === augmented assignment ===
x = INT_MIN
x //= -1
assert x == 9223372036854775808

x = INT_MIN
x %= -1
assert x == 0

# === i64 boundary values ===
assert INT_MIN // 1 == INT_MIN
assert INT_MIN // -2 == 4611686018427387904
assert INT_MIN % 1 == 0
assert INT_MIN % -2 == 0
assert INT_MIN % 3 == 1

assert INT_MAX // -1 == -INT_MAX
assert INT_MAX % -1 == 0
assert INT_MAX // 2 == 4611686018427387903
assert INT_MAX % 2 == 1

# === boundary divisors ===
assert INT_MIN // INT_MIN == 1
assert INT_MIN // INT_MAX == -2
assert INT_MAX // INT_MIN == -1
assert INT_MIN % INT_MIN == 0
assert INT_MAX % INT_MAX == 0

# === sign combinations ===
assert -7 // 2 == -4
assert 7 // -2 == -4
assert -7 % 2 == 1
assert 7 % -2 == -1

q, r = divmod(-7, 2)
assert q == -4
assert r == 1

q, r = divmod(7, -2)
assert q == -4
assert r == -1

# === divmod at boundaries ===
q, r = divmod(INT_MIN, 2)
assert q == -4611686018427387904
assert r == 0

q, r = divmod(INT_MAX, -1)
assert q == -INT_MAX
assert r == 0

q, r = divmod(INT_MIN, INT_MAX)
assert q == -2
assert r == INT_MAX - 1

# === divmod invariant: q * b + r == a ===
q, r = divmod(INT_MIN, -1)
assert q * -1 + r == INT_MIN

q, r = divmod(INT_MIN, 3)
assert q * 3 + r == INT_MIN
assert q == -3074457345618258603
assert r == 1

# === CompareModEq patterns ===
x = INT_MIN
assert x % -1 == 0
assert x % 2 == 0
assert x % 3 == 1

x = INT_MAX
assert x % -1 == 0
assert x % 2 == 1

# Float remainders must compare exactly with large integer constants.
assert 9007199254740992.0 % 9007199254740994.0 == 9007199254740992
assert not (9007199254740992.0 % 9007199254740994.0 == 9007199254740993)
assert not (9007199254740992.0 % 9007199254740994 == 9007199254740993)
assert not (9007199254740992 % 9007199254740994.0 == 9007199254740993)
