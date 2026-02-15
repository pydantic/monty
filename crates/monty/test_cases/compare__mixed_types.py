# === Bool == Int equality ===
assert True == 1, 'True == 1'
assert False == 0, 'False == 0'
assert 1 == True, '1 == True'
assert 0 == False, '0 == False'
assert True != 2, 'True != 2'
assert False != 1, 'False != 1'

# === Bool == Float equality ===
assert True == 1.0, 'True == 1.0'
assert False == 0.0, 'False == 0.0'
assert 1.0 == True, '1.0 == True'
assert 0.0 == False, '0.0 == False'
assert True != 2.0, 'True != 2.0'
assert 0.5 != False, '0.5 != False'

# === Int == Float equality ===
assert 5 == 5.0, '5 == 5.0'
assert 5.0 == 5, '5.0 == 5'
assert 5 != 5.5, '5 != 5.5'
assert 0 == 0.0, '0 == 0.0'
assert -3 == -3.0, '-3 == -3.0'

# === Int/Float ordering ===
assert 5 < 5.5, '5 < 5.5'
assert 5.5 > 5, '5.5 > 5'
assert 5 <= 5.0, '5 <= 5.0'
assert 5.0 >= 5, '5.0 >= 5'
assert 5 > 4.9, '5 > 4.9'
assert 4.9 < 5, '4.9 < 5'

# === Bool ordering (promotes to int) ===
assert True > False, 'True > False'
assert False < True, 'False < True'
assert True >= 1, 'True >= 1'
assert False <= 0, 'False <= 0'
assert True > 0, 'True > 0'
assert True < 2, 'True < 2'
assert True > 0.5, 'True > 0.5'
assert True < 1.5, 'True < 1.5'
assert False < 0.5, 'False < 0.5'
assert False >= -1, 'False >= -1'

# === Cross-type non-equality ===
assert 'hello' != 42, 'str != int'
assert 42 != 'hello', 'int != str'
assert b'hello' != 'hello', 'bytes != str'
assert 'hello' != b'hello', 'str != bytes'
assert None != 0, 'None != 0'
assert 0 != None, '0 != None'
assert [] != 'list', 'list != str'
assert {} != 0, 'dict != int'

# === LongInt cross-type comparisons ===
big = 2**100
big2 = 2**100
assert big == big2, 'LongInt == LongInt'
assert big != 5, 'LongInt != int'
assert big > 5, 'LongInt > int'
assert 5 < big, 'int < LongInt'
assert big >= 5, 'LongInt >= int'
assert 5 <= big, 'int <= LongInt'
small_big = 2**100
large_big = 2**101
assert small_big < large_big, 'LongInt < LongInt'
assert large_big > small_big, 'LongInt > LongInt'
assert big != 'hello', 'LongInt != str'

# === Bytes ordering ===
assert b'abc' < b'abd', 'bytes lt'
assert b'abc' <= b'abc', 'bytes le'
assert b'abd' > b'abc', 'bytes gt'
assert b'abc' >= b'abc', 'bytes ge'
assert b'a' < b'b', 'single byte lt'
assert b'' < b'a', 'empty bytes lt non-empty'

# === String ordering ===
assert 'abc' < 'abd', 'str lt'
assert 'abc' <= 'abc', 'str le'
assert 'abd' > 'abc', 'str gt'
assert 'abc' >= 'abc', 'str ge'
assert 'a' < 'b', 'single char lt'

# === Heap-allocated string ordering (from split) ===
parts = 'banana,apple'.split(',')
assert parts[1] < parts[0], 'heap str lt'
assert parts[0] > parts[1], 'heap str gt'
assert parts[0] >= parts[0], 'heap str ge self'
assert parts[0] <= parts[0], 'heap str le self'

# === Cross-type string ordering (interned vs heap) ===
heap_str = 'banana,apple'.split(',')[0]
assert heap_str > 'apple', 'heap str gt interned'
assert 'cherry' > heap_str, 'interned gt heap str'
assert heap_str >= 'banana', 'heap str ge interned eq'
assert 'banana' <= heap_str, 'interned le heap str eq'

# === Containment: not in list ===
assert 999 not in [1, 2, 3], 'not in list'
assert 0 not in [1, 2, 3], 'zero not in list'

# === Containment: not in tuple ===
assert 'z' not in ('a', 'b', 'c'), 'not in tuple'
assert 0 not in (1, 2, 3), 'zero not in tuple'

# === Containment: in/not in set ===
assert 2 in {1, 2, 3}, 'in set'
assert 99 not in {1, 2, 3}, 'not in set'

# === Containment: in/not in frozenset ===
assert 2 in frozenset({1, 2, 3}), 'in frozenset'
assert 99 not in frozenset({1, 2, 3}), 'not in frozenset'

# === Containment: in/not in list (found) ===
assert 2 in [1, 2, 3], 'in list'
assert 'b' in ['a', 'b', 'c'], 'str in list'

# === Containment: in/not in tuple (found) ===
assert 'b' in ('a', 'b', 'c'), 'str in tuple'
assert 2 in (1, 2, 3), 'int in tuple'
