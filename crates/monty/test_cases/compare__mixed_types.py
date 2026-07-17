# === Bool == Int equality ===
assert True == 1
assert False == 0
assert 1 == True
assert 0 == False
assert True != 2
assert False != 1

# === Bool == Float equality ===
assert True == 1.0
assert False == 0.0
assert 1.0 == True
assert 0.0 == False
assert True != 2.0
assert 0.5 != False

# === Int == Float equality ===
assert 5 == 5.0
assert 5.0 == 5
assert 5 != 5.5
assert 0 == 0.0
assert -3 == -3.0

# === Int/Float ordering ===
assert 5 < 5.5
assert 5.5 > 5
assert 5 <= 5.0
assert 5.0 >= 5
assert 5 > 4.9
assert 4.9 < 5

# === Bool ordering (promotes to int) ===
assert True > False
assert False < True
assert True >= 1
assert False <= 0
assert True > 0
assert True < 2
assert True > 0.5
assert True < 1.5
assert False < 0.5
assert False >= -1

# === Cross-type non-equality ===
assert 'hello' != 42
assert 42 != 'hello'
assert b'hello' != 'hello'
assert 'hello' != b'hello'
assert None != 0
assert 0 != None
assert [] != 'list'
assert {} != 0

# === LongInt cross-type comparisons ===
big = 2**100
big2 = 2**100
assert big == big2
assert big != 5
assert big > 5
assert 5 < big
assert big >= 5
assert 5 <= big
small_big = 2**100
large_big = 2**101
assert small_big < large_big
assert large_big > small_big
assert big != 'hello'

# === Float vs LongInt comparisons (exact, no precision loss) ===
# Powers of two are exactly representable as f64, so these are exactly equal
assert 2.0**100 == 2**100
assert 2**100 == 2.0**100
assert 2.0**100 != 2**100 + 1
assert 2**100 + 1 != 2.0**100
# Non-power-of-two big ints are not exactly representable; comparison is still exact
assert 1e30 != 10**30
assert 10**30 != 1e30
# Non-integral float is never equal to any int
assert 2.5 != 2**100
assert 2**100 != 2.5
# Ordering across float and LongInt
assert 2.0**100 < 2**101
assert 2**101 > 2.0**100
assert 1e308 < 10**400
assert 10**400 > 1e308
assert 2.5 < 2**100
assert 2**100 > 2.5
# Infinities compare against LongInt without overflow
assert float('inf') > 10**400
assert 10**400 < float('inf')
assert float('-inf') < 10**400
assert 10**400 > float('-inf')

# Equal float/LongInt pairs must hash equally and be interchangeable dict keys
assert hash(2.0**100) == hash(2**100)
assert {2**100: 'a'}[2.0**100] == 'a'
assert {2.0**100: 'b'}[2**100] == 'b'
assert 2.0**100 in {2**100, 3}

# === Bytes ordering ===
assert b'abc' < b'abd'
assert b'abc' <= b'abc'
assert b'abd' > b'abc'
assert b'abc' >= b'abc'
assert b'a' < b'b'
assert b'' < b'a'

# === String ordering ===
assert 'abc' < 'abd'
assert 'abc' <= 'abc'
assert 'abd' > 'abc'
assert 'abc' >= 'abc'
assert 'a' < 'b'

# === Heap-allocated string ordering (from split) ===
parts = 'banana,apple'.split(',')
assert parts[1] < parts[0]
assert parts[0] > parts[1]
assert parts[0] >= parts[0]
assert parts[0] <= parts[0]

# === Cross-type string ordering (interned vs heap) ===
heap_str = 'banana,apple'.split(',')[0]
assert heap_str > 'apple'
assert 'cherry' > heap_str
assert heap_str >= 'banana'
assert 'banana' <= heap_str

# === Containment: not in list ===
assert 999 not in [1, 2, 3]
assert 0 not in [1, 2, 3]

# === Containment: not in tuple ===
assert 'z' not in ('a', 'b', 'c')
assert 0 not in (1, 2, 3)

# === Containment: in/not in set ===
assert 2 in {1, 2, 3}
assert 99 not in {1, 2, 3}

# === Containment: in/not in frozenset ===
assert 2 in frozenset({1, 2, 3})
assert 99 not in frozenset({1, 2, 3})

# === Containment: in/not in list (found) ===
assert 2 in [1, 2, 3]
assert 'b' in ['a', 'b', 'c']

# === Containment: in/not in tuple (found) ===
assert 'b' in ('a', 'b', 'c')
assert 2 in (1, 2, 3)

# === List ordering (lexicographic, like tuples) ===
assert [1, 2] < [1, 3]
assert [1, 2, 3] > [1, 2]
assert [1] < [1, 2]
assert [1, 2] <= [1, 2]
assert [1, 2] >= [1, 2]
assert ['a', 'b'] < ['a', 'c']
assert [[1], [2]] < [[1], [3]]
assert not ([2] < [1]), 'list not lt'
# equal-but-unorderable elements don't block ordering (None == None), like CPython
assert [None, 1] < [None, 2]

# === Unorderable comparisons raise TypeError (not silently False) ===
try:
    1 < 'a'
    assert False, 'expected int < str to raise'
except TypeError as exc:
    assert str(exc) == "'<' not supported between instances of 'int' and 'str'"

try:
    None < None
    assert False, 'expected None < None to raise'
except TypeError as exc:
    assert str(exc) == "'<' not supported between instances of 'NoneType' and 'NoneType'"

try:
    [1] >= 'a'
    assert False, 'expected list >= str to raise'
except TypeError as exc:
    assert str(exc) == "'>=' not supported between instances of 'list' and 'str'"

# === NaN ordering returns False, never raises (NaN is unordered, not incomparable) ===
# CPython: every ordering operator against a NaN yields False, for both directions
# and for two NaNs. This is distinct from a type mismatch, which raises.
nan = float('nan')
assert not (nan < 1), 'nan < int is False'
assert not (nan <= 1), 'nan <= int is False'
assert not (nan > 1), 'nan > int is False'
assert not (nan >= 1), 'nan >= int is False'
assert not (1 < nan), 'int < nan is False'
assert not (1 >= nan), 'int >= nan is False'
assert not (nan < nan), 'nan < nan is False'
assert not (nan >= nan), 'nan >= nan is False'
assert not (nan < 10**30), 'nan < LongInt is False'
assert not (nan < 1.5), 'nan < float is False'
assert not (nan < True), 'nan < bool is False'

# NaN combined with a genuine type mismatch still raises (numeric-vs-str)
try:
    nan < 'a'
    assert False, 'expected nan < str to raise'
except TypeError as exc:
    assert str(exc) == "'<' not supported between instances of 'float' and 'str'"

# === NaN inside containers ===
# The first differing element decides: a NaN element makes the container unordered
# (False), a type-mismatched element makes it incomparable (raises).
assert not ([nan] < [1]), 'list with nan element is unordered'
assert not ((nan,) < (1,)), 'tuple with nan element is unordered'
assert not ([1, nan] < [1, 2]), 'nan as second element is unordered'
try:
    [nan] < ['a']
    assert False, 'expected list with float/str element mismatch to raise'
except TypeError:
    # Both raise TypeError; only the message text diverges (Monty names the
    # outer 'list'/'list', CPython the inner 'float'/'str') — see
    # limitations/language.md, so the exact message is not asserted here.
    pass

# === sorted / min / max with NaN do not raise ===
# NaN compares as neither less nor greater, so it never triggers a swap; CPython
# leaves such elements in place rather than erroring.
assert sorted([3, 1, 2]) == [1, 2, 3]
assert sorted([nan, 1, 2, nan])[1:3] == [1, 2]
assert min([nan, 1, 2]) != min([nan, 1, 2])
assert max([1, nan, 2]) == 2
assert min([1, 2, nan]) == 1
