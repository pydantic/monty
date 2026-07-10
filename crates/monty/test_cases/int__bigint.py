# Tests for BigInt (arbitrary precision integer) support

# === Setup constants ===
MAX_I64 = 9223372036854775807  # i64::MAX
MIN_I64 = -MAX_I64 - 1  # i64::MIN (compute to avoid type checker overflow)

# === BigInt literals ===
# Monty supports parsing integer literals larger than i64
LITERAL_BIG = 10000000000000000000000000000000000000000
assert LITERAL_BIG == 10**40
assert str(LITERAL_BIG) == '10000000000000000000000000000000000000000'
assert type(LITERAL_BIG) == int

# Negative bigint literal (via unary negation)
NEG_BIG_LITERAL = -10000000000000000000000000000000000000000
assert NEG_BIG_LITERAL == -(10**40)
assert str(NEG_BIG_LITERAL) == '-10000000000000000000000000000000000000000'

# === BigInt literal arithmetic ===
# bigint_literal * int
assert 10000000000000000000000000000000000000000 * 2 == 2 * 10**40
assert 2 * 10000000000000000000000000000000000000000 == 2 * 10**40

# bigint_literal / int (true division)
assert 10000000000000000000000000000000000000000 / 2 == 10**40 / 2
assert 10000000000000000000000000000000000000000 / 10000000000000000000000000000000000000000 == 1.0

# bigint_literal // int (floor division)
assert 10000000000000000000000000000000000000000 // 3 == 10**40 // 3
assert 10000000000000000000000000000000000000000 // 10000000000000000000000000000000000000000 == 1

# bigint_literal % int (modulo)
assert 10000000000000000000000000000000000000000 % 7 == 10**40 % 7
assert 10000000000000000000000000000000000000001 % 10000000000000000000000000000000000000000 == 1

# bigint_literal + int
assert 10000000000000000000000000000000000000000 + 1 == 10**40 + 1
assert 1 + 10000000000000000000000000000000000000000 == 10**40 + 1

# bigint_literal - int
assert 10000000000000000000000000000000000000000 - 1 == 10**40 - 1
assert 10000000000000000000000000000000000000001 - 10000000000000000000000000000000000000000 == 1

# bigint_literal ** int
assert 10000000000000000000**2 == 10**38

# === int() parsing of big integers ===
assert int('10000000000000000000000000000000000000000') == 10**40
assert int('-10000000000000000000000000000000000000000') == -(10**40)
assert int('99999999999999999999999999999999999999999999999999') == 10**50 - 1

# === BigInt literal comparisons ===
assert 10000000000000000000000000000000000000000 > 9999999999999999999999999999999999999999
assert 10000000000000000000000000000000000000000 >= 10000000000000000000000000000000000000000
assert 9999999999999999999999999999999999999999 < 10000000000000000000000000000000000000000
assert 10000000000000000000000000000000000000000 <= 10000000000000000000000000000000000000000
assert 10000000000000000000000000000000000000000 == 10000000000000000000000000000000000000000
assert 10**40 == 10000000000000000000000000000000000000000
assert 10000000000000000000000000000000000000000 == 10**40
assert 1267650600228229401496703205376 == 2.0**100
assert 2.0**100 == 1267650600228229401496703205376
assert 10000000000000000000000000000000000000000 != 10000000000000000000000000000000000000001

# bigint literal vs int comparisons
assert 10000000000000000000000000000000000000000 > 1
assert 1 < 10000000000000000000000000000000000000000

# === BigInt literal bool conversion ===
assert bool(10000000000000000000000000000000000000000)
assert bool(-10000000000000000000000000000000000000000)

# === BigInt literal hash consistency ===
# Same literal value should have same hash
h1 = hash(10000000000000000000000000000000000000000)
h2 = hash(10000000000000000000000000000000000000000)
assert h1 == h2

# Computed equal value should have same hash
h3 = hash(10**40)
assert h1 == h3

# === BigInt literal bitwise operations ===
assert 10000000000000000000000000000000000000000 & 0xFF == (10**40) & 0xFF
assert 10000000000000000000000000000000000000000 | 1 == (10**40) | 1
assert 10000000000000000000000000000000000000000 ^ 10000000000000000000000000000000000000000 == 0
assert 10000000000000000000000000000000000000000 >> 10 == (10**40) >> 10
assert 10000000000000000000000000000000000000000 << 10 == (10**40) << 10

# === Non-decimal BigInt literals ===
# Large hex literal (2^64)
big_hex = 0x10000000000000000
assert big_hex == 2**64

bigger_hex = 0x10000000000000000123
assert bigger_hex == 75557863725914323419427, f'large hex literal {bigger_hex}'

# Large binary literal (2^65)
big_bin = 0b100000000000000000000000000000000000000000000000000000000000000000
assert big_bin == 2**65

# Large octal literal
big_oct = 0o10000000000000000000000
assert big_oct == 8**22

# Underscores in large non-decimal
big_hex_underscore = 0x1_0000_0000_0000_0000
assert big_hex_underscore == 2**64

# === BigInt literal in collections ===
d = {10000000000000000000000000000000000000000: 'value'}
assert d[10000000000000000000000000000000000000000] == 'value'
assert d[10**40] == 'value'

lst = [10000000000000000000000000000000000000000, 20000000000000000000000000000000000000000]
assert lst[0] == 10**40
assert lst[1] == 2 * 10**40

# === BigInt literal repr/str ===
assert repr(10000000000000000000000000000000000000000) == '10000000000000000000000000000000000000000'
assert str(10000000000000000000000000000000000000000) == '10000000000000000000000000000000000000000'

# === Overflow promotion ===
bigger = MAX_I64 + 1
assert bigger == MAX_I64 + 1
assert bigger - 1 == MAX_I64

# === Subtraction overflow ===
smaller = MIN_I64 - 1
assert smaller == MIN_I64 - 1
assert smaller + 1 == MIN_I64

# === Multiplication overflow ===
mul_result = MAX_I64 * 2
expected_mul = MAX_I64 + MAX_I64
assert mul_result == expected_mul
trillion = 1000000000000
trillion_squared = trillion * trillion
assert trillion_squared == 1000000000000 * 1000000000000

# === Power overflow ===
pow_2_63 = 2**63
assert pow_2_63 == MAX_I64 + 1
pow_2_64 = 2**64
assert pow_2_64 == pow_2_63 * 2
pow_2_100 = 2**100
assert pow_2_100 > pow_2_64

# === Negative overflow ===
neg_bigger = -MAX_I64 - 2
assert neg_bigger == MIN_I64 - 1

# === Type is still int ===
assert type(bigger) == int
assert type(pow_2_100) == int

# === Mixed operations ===
add_result = bigger + 100
assert add_result == MAX_I64 + 101
add_result2 = 100 + bigger
assert add_result2 == MAX_I64 + 101
sub_result = bigger - 100
assert sub_result == MAX_I64 - 99
sub_result2 = 100 - bigger
expected_sub = -(MAX_I64 - 99)
assert sub_result2 == expected_sub
mul_result2 = bigger * 2
expected_mul2 = (MAX_I64 + 1) * 2
assert mul_result2 == expected_mul2
mul_result3 = 2 * bigger
assert mul_result3 == expected_mul2

# === BigInt with BigInt operations ===
big_a = 2**100
big_b = 2**100
big_sum = big_a + big_b
assert big_sum == 2**101
big_diff = big_a - big_b
assert big_diff == 0
big_prod = big_a * big_b
assert big_prod == 2**200

# === Comparisons ===
assert bigger > MAX_I64
assert MAX_I64 < bigger
assert bigger >= MAX_I64
assert MAX_I64 <= bigger
cmp_result = bigger == MAX_I64 + 1
assert cmp_result
cmp_result2 = bigger == MAX_I64
assert not cmp_result2, 'bigint != int'

# === BigInt comparisons ===
assert big_a == big_b
cmp_lt = big_a < big_b
assert not cmp_lt, 'bigint not < equal bigint'
big_double = big_a * 2
assert big_double > big_b

# === Hash consistency ===
# When a BigInt demotes to i64 range, its hash must match the equivalent int hash
# This is critical for dict key lookups to work correctly

# Test hash equality for values that fit in i64
computed_42 = (big_a - big_a) + 42  # Goes through BigInt arithmetic, demotes to 42
assert hash(computed_42) == hash(42)
assert hash(bigger - 1) == hash(MAX_I64)
assert hash(smaller + 1) == hash(MIN_I64)

# Test that hash(0) is consistent across computation paths
zero_via_bigint = big_a - big_a
assert hash(zero_via_bigint) == hash(0)

# Test dict key lookup works when inserting with int and looking up with computed bigint
d = {42: 'a'}
assert d[42] == 'a'
assert d[computed_42] == 'a'

# Test dict key lookup works when inserting with bigint and looking up with int
d2 = {computed_42: 'value'}
assert d2[42] == 'value'

# Large bigints (outside i64 range) as dict keys
d[bigger] = 'b'
assert d[bigger] == 'b'
d[big_a] = 'c'
assert d[big_a] == 'c'

# Verify large bigints with same value hash the same
big_copy = 2**100
assert hash(big_a) == hash(big_copy)

# Verify large bigints can be used interchangeably as dict keys
d3 = {big_a: 'original'}
assert d3[big_copy] == 'original'

# === Unary neg overflow ===
# Use 0 - MIN_I64 instead of -MIN_I64 to avoid type checker overflow
neg_min = 0 - MIN_I64
assert neg_min == MAX_I64 + 1

# Note: ~bigger (bitwise not) tests skipped - Monty parser doesn't support ~ yet

# === Floor division ===
fd_result = bigger // 2
fd_expected = (MAX_I64 + 1) // 2
assert fd_result == fd_expected
pow_2_50 = 2**50
fd_result2 = pow_2_100 // pow_2_50
assert fd_result2 == 2**50
fd_result3 = 100 // bigger
assert fd_result3 == 0
neg_bigger = -bigger
fd_neg_result = neg_bigger // 3
fd_neg_expected = (-(MAX_I64 + 1)) // 3
assert fd_neg_result == fd_neg_expected

# === Modulo ===
mod_result = bigger % 1000
mod_expected = (MAX_I64 + 1) % 1000
assert mod_result == mod_expected
mod_result2 = 100 % bigger
assert mod_result2 == 100
mod_result3 = pow_2_100 % (pow_2_50 + 1)
assert mod_result3 == 1

# === Builtin functions ===
abs_neg = abs(-bigger)
assert abs_neg == bigger
abs_pos = abs(bigger)
assert abs_pos == bigger
abs_min = abs(MIN_I64)
assert abs_min == MAX_I64 + 1

pow_result = pow(2, 100)
assert pow_result == pow_2_100
pow_bigger_2 = bigger * bigger
pow_result2 = pow(bigger, 2)
assert pow_result2 == pow_bigger_2

dm = divmod(bigger, 1000)
dm_quot = dm[0]
dm_rem = dm[1]
expected_quot = bigger // 1000
expected_rem = bigger % 1000
assert dm_quot == expected_quot
assert dm_rem == expected_rem
dm2 = divmod(pow_2_100, pow_2_50)
assert dm2[0] == pow_2_50
assert dm2[1] == 0

hex_result = hex(bigger)
assert hex_result == '0x8000000000000000'
hex_neg = hex(-bigger)
assert hex_neg == '-0x8000000000000000'

bin_result = bin(bigger)
assert bin_result == '0b1000000000000000000000000000000000000000000000000000000000000000'
bin_neg = bin(-bigger)
assert bin_neg == '-0b1000000000000000000000000000000000000000000000000000000000000000'

oct_result = oct(bigger)
assert oct_result == '0o1000000000000000000000'
oct_neg = oct(-bigger)
assert oct_neg == '-0o1000000000000000000000'

# === Repr and str ===
repr_result = repr(bigger)
str_result = str(bigger)
expected_repr = str(MAX_I64 + 1)
assert repr_result == expected_repr
assert str_result == expected_repr

# === Bool conversion ===
assert bool(bigger)
assert bool(-bigger)

# === Demote back to i64 ===
demote_result = bigger - bigger
assert demote_result == 0
demote_result2 = bigger - 1
assert demote_result2 == MAX_I64

# === Bug 1: 0 ** 0 with LongInt exponent ===
big = 2**100
assert 0**big == 0
assert 1**big == 1
# Edge case: 0 ** 0 where 0 is a LongInt
zero_big = big - big  # LongInt zero (actually demotes to int, so test with computed zero)
assert 0**zero_big == 1
assert 5**zero_big == 1

# === Bug 2: Modulo with negative divisor ===
assert 5 % -3 == -1
assert -5 % 3 == 1
assert -5 % -3 == -2
assert 7 % -4 == -1

# === Bug 3: += overflow ===
x = MAX_I64
x += 1
assert x == MAX_I64 + 1
y = MIN_I64
y += -1
assert y == MIN_I64 - 1

# === Bug 4: LongInt * sequence ===
big = 2**100
assert 'a' * 0 == ''
assert [1] * 0 == []
# Sequence * LongInt (where LongInt is heap-allocated)
# Note: CPython doesn't support seq * huge_negative_longint (OverflowError)
# Test with positive LongInt - should raise OverflowError for repeat count too large
# But we can test heap-allocated LongInt by using a value that demotes
big_then_small = big - big + 3  # Results in 3 (goes through LongInt arithmetic)
assert 'ab' * big_then_small == 'ababab'

# === Bug 5: True division with LongInt ===
big = 2**100
assert big / 2 == 2.0**99
# 1 / 2**100 is a very small positive number, not exactly 0.0
tiny = 1 / big
assert tiny > 0.0 and tiny < 1e-29, 'int / huge_bigint approaches 0'
assert big / big == 1.0
assert big / 2.0 == 2.0**99
tiny_f = 1.0 / big
assert tiny_f > 0.0 and tiny_f < 1e-29, 'float / huge_bigint approaches 0'

# === Bug 6: Bitwise with LongInt ===
big = 2**100
assert big & 0xFF == 0
assert big | 1 == big + 1
assert big ^ big == 0
assert big >> 50 == 2**50
assert 1 << 100 == big
assert (big + 0xFF) & 0xFF == 0xFF

# === Large result operations (should succeed with NoLimitTracker) ===
# These are large but allowed since test runner uses NoLimitTracker
x = 2**100000  # ~12.5KB - well under any reasonable limit
assert x > 0

y = 1 << 100000
assert y > 0

# Edge cases (constant-size results) - always succeed
assert 0**10000000 == 0
assert 1**10000000 == 1
assert (-1) ** 10000000 == 1
assert (-1) ** 10000001 == -1
assert 0 << 10000000 == 0

# === LongInt in range() ===
# Note: Monty raises OverflowError immediately for range(10**100), while CPython
# only raises when iterating or calling len(). We accept this difference for safety.
big = 2**100
small_via_big = big - big + 5  # LongInt that demotes to 5
r = range(small_via_big)
assert list(r) == [0, 1, 2, 3, 4]

r2 = range(small_via_big, small_via_big + 3)
assert list(r2) == [5, 6, 7]

r3 = range(0, 10, big - big + 2)
assert list(r3) == [0, 2, 4, 6, 8]

# === Integer computed via LongInt arithmetic ===
# These values go through BigInt arithmetic but demote to regular Int via into_value()
idx = big - big + 1  # Results in Value::Int(1) after demotion
assert [10, 20, 30][idx] == 20
assert (10, 20, 30)[idx] == 20
assert 'abc'[idx] == 'b'
assert b'abc'[idx] == ord('b')
assert range(10)[idx] == 1

# Negative index computed via LongInt arithmetic
neg_idx = big - big - 1  # Results in Value::Int(-1) after demotion
assert [10, 20, 30][neg_idx] == 30
assert (10, 20, 30)[neg_idx] == 30
assert 'abc'[neg_idx] == 'c'
assert b'abc'[neg_idx] == ord('c')
assert range(10)[neg_idx] == 9

# List assignment with LongInt index
lst = [1, 2, 3]
lst[idx] = 42
assert lst == [1, 42, 3]
lst[neg_idx] = 99
assert lst == [1, 42, 99]

# === String/bytes * LongInt ===
count = big - big + 3
assert 'ab' * count == 'ababab'
assert count * 'ab' == 'ababab'
assert b'ab' * count == b'ababab'
assert count * b'ab' == b'ababab'

# Negative LongInt repeat
neg = big - big - 2
assert 'ab' * neg == ''
assert b'ab' * neg == b''

# Zero LongInt repeat
zero = big - big
assert 'ab' * zero == ''
assert b'ab' * zero == b''
