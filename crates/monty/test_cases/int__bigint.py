# Tests for BigInt (arbitrary precision integer) support
# Note: Monty's parser doesn't support literals > i64, so we compute large values

# === Setup constants ===
MAX_I64 = 9223372036854775807  # i64::MAX
MIN_I64 = -MAX_I64 - 1  # i64::MIN (compute to avoid type checker overflow)

# === Overflow promotion ===
bigger = MAX_I64 + 1
assert bigger == MAX_I64 + 1, 'add overflow promotes to bigint'
assert bigger - 1 == MAX_I64, 'sub back to i64'

# === Subtraction overflow ===
smaller = MIN_I64 - 1
assert smaller == MIN_I64 - 1, 'sub overflow promotes to bigint'
assert smaller + 1 == MIN_I64, 'add back to i64'

# === Multiplication overflow ===
mul_result = MAX_I64 * 2
expected_mul = MAX_I64 + MAX_I64
assert mul_result == expected_mul, 'mul overflow'
trillion = 1000000000000
trillion_squared = trillion * trillion
assert trillion_squared == 1000000000000 * 1000000000000, 'large mul'

# === Power overflow ===
pow_2_63 = 2**63
assert pow_2_63 == MAX_I64 + 1, 'pow creates bigint at boundary'
pow_2_64 = 2**64
assert pow_2_64 == pow_2_63 * 2, 'pow overflow'
pow_2_100 = 2**100
assert pow_2_100 > pow_2_64, 'large pow is greater'

# === Negative overflow ===
neg_bigger = -MAX_I64 - 2
assert neg_bigger == MIN_I64 - 1, 'negative bigint'

# === Type is still int ===
assert type(bigger) == int, 'bigint type is int'
assert type(pow_2_100) == int, 'large pow type is int'

# === Mixed operations ===
add_result = bigger + 100
assert add_result == MAX_I64 + 101, 'bigint + int'
add_result2 = 100 + bigger
assert add_result2 == MAX_I64 + 101, 'int + bigint'
sub_result = bigger - 100
assert sub_result == MAX_I64 - 99, 'bigint - int'
sub_result2 = 100 - bigger
expected_sub = -(MAX_I64 - 99)
assert sub_result2 == expected_sub, 'int - bigint'
mul_result2 = bigger * 2
expected_mul2 = (MAX_I64 + 1) * 2
assert mul_result2 == expected_mul2, 'bigint * int'
mul_result3 = 2 * bigger
assert mul_result3 == expected_mul2, 'int * bigint'

# === BigInt with BigInt operations ===
big_a = 2**100
big_b = 2**100
big_sum = big_a + big_b
assert big_sum == 2**101, 'bigint + bigint'
big_diff = big_a - big_b
assert big_diff == 0, 'bigint - bigint'
big_prod = big_a * big_b
assert big_prod == 2**200, 'bigint * bigint'

# === Comparisons ===
assert bigger > MAX_I64, 'bigint > int'
assert MAX_I64 < bigger, 'int < bigint'
assert bigger >= MAX_I64, 'bigint >= int'
assert MAX_I64 <= bigger, 'int <= bigint'
cmp_result = bigger == MAX_I64 + 1
assert cmp_result, 'bigint == computed int'
cmp_result2 = bigger == MAX_I64
assert not cmp_result2, 'bigint != int'

# === BigInt comparisons ===
assert big_a == big_b, 'bigint == bigint'
cmp_lt = big_a < big_b
assert not cmp_lt, 'bigint not < equal bigint'
big_double = big_a * 2
assert big_double > big_b, 'larger bigint > smaller bigint'

# === Hash consistency ===
# When a BigInt demotes to i64 range, its hash must match the equivalent int hash
# This is critical for dict key lookups to work correctly

# Test hash equality for values that fit in i64
computed_42 = (big_a - big_a) + 42  # Goes through BigInt arithmetic, demotes to 42
assert hash(computed_42) == hash(42), 'hash of computed int must match literal int'
assert hash(bigger - 1) == hash(MAX_I64), 'hash of demoted bigint must match MAX_I64'
assert hash(smaller + 1) == hash(MIN_I64), 'hash of demoted bigint must match MIN_I64'

# Test that hash(0) is consistent across computation paths
zero_via_bigint = big_a - big_a
assert hash(zero_via_bigint) == hash(0), 'hash of bigint zero must match int zero'

# Test dict key lookup works when inserting with int and looking up with computed bigint
d = {42: 'a'}
assert d[42] == 'a', 'int as key'
assert d[computed_42] == 'a', 'lookup with computed bigint finds int key'

# Test dict key lookup works when inserting with bigint and looking up with int
d2 = {computed_42: 'value'}
assert d2[42] == 'value', 'lookup with int finds bigint key'

# Large bigints (outside i64 range) as dict keys
d[bigger] = 'b'
assert d[bigger] == 'b', 'bigint as key'
d[big_a] = 'c'
assert d[big_a] == 'c', 'large bigint as key'

# Verify large bigints with same value hash the same
big_copy = 2**100
assert hash(big_a) == hash(big_copy), 'equal large bigints must hash the same'

# Verify large bigints can be used interchangeably as dict keys
d3 = {big_a: 'original'}
assert d3[big_copy] == 'original', 'lookup with equal large bigint works'

# === Unary neg overflow ===
# Use 0 - MIN_I64 instead of -MIN_I64 to avoid type checker overflow
neg_min = 0 - MIN_I64
assert neg_min == MAX_I64 + 1, 'neg i64::MIN promotes'

# Note: ~bigger (bitwise not) tests skipped - Monty parser doesn't support ~ yet

# === Floor division ===
fd_result = bigger // 2
fd_expected = (MAX_I64 + 1) // 2
assert fd_result == fd_expected, 'bigint // int'
pow_2_50 = 2**50
fd_result2 = pow_2_100 // pow_2_50
assert fd_result2 == 2**50, 'bigint // bigint'
fd_result3 = 100 // bigger
assert fd_result3 == 0, 'int // bigint (small / large)'
neg_bigger = -bigger
fd_neg_result = neg_bigger // 3
fd_neg_expected = (-(MAX_I64 + 1)) // 3
assert fd_neg_result == fd_neg_expected, 'negative bigint floordiv'

# === Modulo ===
mod_result = bigger % 1000
mod_expected = (MAX_I64 + 1) % 1000
assert mod_result == mod_expected, 'bigint % int'
mod_result2 = 100 % bigger
assert mod_result2 == 100, 'int % bigint'
mod_result3 = pow_2_100 % (pow_2_50 + 1)
assert mod_result3 == 1, 'bigint % bigint'

# === Builtin functions ===
abs_neg = abs(-bigger)
assert abs_neg == bigger, 'abs of negative bigint'
abs_pos = abs(bigger)
assert abs_pos == bigger, 'abs of positive bigint'
abs_min = abs(MIN_I64)
assert abs_min == MAX_I64 + 1, 'abs of i64::MIN'

pow_result = pow(2, 100)
assert pow_result == pow_2_100, 'pow builtin'
pow_bigger_2 = bigger * bigger
pow_result2 = pow(bigger, 2)
assert pow_result2 == pow_bigger_2, 'pow with bigint base'

dm = divmod(bigger, 1000)
dm_quot = dm[0]
dm_rem = dm[1]
expected_quot = bigger // 1000
expected_rem = bigger % 1000
assert dm_quot == expected_quot, 'divmod quotient with bigint'
assert dm_rem == expected_rem, 'divmod remainder with bigint'
dm2 = divmod(pow_2_100, pow_2_50)
assert dm2[0] == pow_2_50, 'divmod bigint by bigint quotient'
assert dm2[1] == 0, 'divmod bigint by bigint remainder'

hex_result = hex(bigger)
assert hex_result == '0x8000000000000000', 'hex of bigint'
hex_neg = hex(-bigger)
assert hex_neg == '-0x8000000000000000', 'hex of negative bigint'

bin_result = bin(bigger)
assert bin_result == '0b1000000000000000000000000000000000000000000000000000000000000000', 'bin of bigint'
bin_neg = bin(-bigger)
assert bin_neg == '-0b1000000000000000000000000000000000000000000000000000000000000000', 'bin of negative bigint'

oct_result = oct(bigger)
assert oct_result == '0o1000000000000000000000', 'oct of bigint'
oct_neg = oct(-bigger)
assert oct_neg == '-0o1000000000000000000000', 'oct of negative bigint'

# === Repr and str ===
repr_result = repr(bigger)
str_result = str(bigger)
expected_repr = str(MAX_I64 + 1)
assert repr_result == expected_repr, 'repr of bigint'
assert str_result == expected_repr, 'str of bigint'

# === Bool conversion ===
assert bool(bigger), 'bigint is truthy'
assert bool(-bigger), 'negative bigint is truthy'

# === Demote back to i64 ===
demote_result = bigger - bigger
assert demote_result == 0, 'bigint - bigint can demote to i64'
demote_result2 = bigger - 1
assert demote_result2 == MAX_I64, 'bigint - 1 demotes to i64::MAX'

# === Bug 1: 0 ** 0 with LongInt exponent ===
big = 2**100
assert 0**big == 0, '0 ** large_positive should be 0'
assert 1**big == 1, '1 ** large_positive should be 1'
# Edge case: 0 ** 0 where 0 is a LongInt
zero_big = big - big  # LongInt zero (actually demotes to int, so test with computed zero)
assert 0**zero_big == 1, '0 ** 0 (computed zero) should be 1'
assert 5**zero_big == 1, '5 ** 0 (computed zero) should be 1'

# === Bug 2: Modulo with negative divisor ===
assert 5 % -3 == -1, '5 % -3 should be -1'
assert -5 % 3 == 1, '-5 % 3 should be 1'
assert -5 % -3 == -2, '-5 % -3 should be -2'
assert 7 % -4 == -1, '7 % -4 should be -1'

# === Bug 3: += overflow ===
x = MAX_I64
x += 1
assert x == MAX_I64 + 1, 'i64::MAX += 1 should promote to LongInt'
y = MIN_I64
y += -1
assert y == MIN_I64 - 1, 'i64::MIN += -1 should promote to LongInt'

# === Bug 4: LongInt * sequence ===
big = 2**100
assert 'a' * 0 == '', 'str * 0'
assert [1] * 0 == [], 'list * 0'
# Sequence * LongInt (where LongInt is heap-allocated)
# Note: CPython doesn't support seq * huge_negative_longint (OverflowError)
# Test with positive LongInt - should raise OverflowError for repeat count too large
# But we can test heap-allocated LongInt by using a value that demotes
big_then_small = big - big + 3  # Results in 3 (goes through LongInt arithmetic)
assert 'ab' * big_then_small == 'ababab', 'str * LongInt that demotes to small value'

# === Bug 5: True division with LongInt ===
big = 2**100
assert big / 2 == 2.0**99, 'bigint / int'
# 1 / 2**100 is a very small positive number, not exactly 0.0
tiny = 1 / big
assert tiny > 0.0 and tiny < 1e-29, 'int / huge_bigint approaches 0'
assert big / big == 1.0, 'bigint / bigint same value'
assert big / 2.0 == 2.0**99, 'bigint / float'
tiny_f = 1.0 / big
assert tiny_f > 0.0 and tiny_f < 1e-29, 'float / huge_bigint approaches 0'

# === Bug 6: Bitwise with LongInt ===
big = 2**100
assert big & 0xFF == 0, '2**100 & 0xFF'
assert big | 1 == big + 1, '2**100 | 1'
assert big ^ big == 0, 'bigint ^ same bigint'
assert big >> 50 == 2**50, '2**100 >> 50'
assert 1 << 100 == big, '1 << 100'
assert (big + 0xFF) & 0xFF == 0xFF, 'bigint with low bits & mask'

# === Large result operations (should succeed with NoLimitTracker) ===
# These are large but allowed since test runner uses NoLimitTracker
x = 2**100000  # ~12.5KB - well under any reasonable limit
assert x > 0, '2 ** 100000 should succeed'

y = 1 << 100000
assert y > 0, '1 << 100000 should succeed'

# Edge cases (constant-size results) - always succeed
assert 0**10000000 == 0, '0 ** huge = 0'
assert 1**10000000 == 1, '1 ** huge = 1'
assert (-1) ** 10000000 == 1, '(-1) ** huge_even = 1'
assert (-1) ** 10000001 == -1, '(-1) ** huge_odd = -1'
assert 0 << 10000000 == 0, '0 << huge = 0'
