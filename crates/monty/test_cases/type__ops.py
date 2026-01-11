# === type() function ===
assert type(1) == int, 'type(int) returns int'
assert type(1.5) == float, 'type(float) returns float'
assert type(True) == bool, 'type(bool) returns bool'
assert type('hello') == str, 'type(str) returns str'
assert type([1, 2]) == list, 'type(list) returns list'
assert type((1, 2)) == tuple, 'type(tuple) returns tuple'
assert type({1: 2}) == dict, 'type(dict) returns dict'
assert type(b'hi') == bytes, 'type(bytes) returns bytes'
assert type(None) == type(None), 'type(None) is consistent'

# === type() inequality ===
assert type(1) != str, 'int type != str'
assert type([]) != tuple, 'list type != tuple'
assert type({}) != list, 'dict type != list'
assert type(1) != float, 'int type != float'

# === type repr ===
assert repr(int) == "<class 'int'>", 'int type repr'
assert repr(float) == "<class 'float'>", 'float type repr'
assert repr(bool) == "<class 'bool'>", 'bool type repr'
assert repr(str) == "<class 'str'>", 'str type repr'
assert repr(list) == "<class 'list'>", 'list type repr'
assert repr(tuple) == "<class 'tuple'>", 'tuple type repr'
assert repr(dict) == "<class 'dict'>", 'dict type repr'
assert repr(bytes) == "<class 'bytes'>", 'bytes type repr'

# === type identity ===
assert int is int, 'int is int'
assert str is str, 'str is str'
assert list is list, 'list is list'
assert type(1) is int, 'type(1) is int'
assert type('') is str, 'type str is str'
assert type([]) is list, 'type([]) is list'

# === list() constructor ===
assert list() == [], 'list() empty'
assert list([1, 2, 3]) == [1, 2, 3], 'list(list) copy'
assert list((1, 2, 3)) == [1, 2, 3], 'list(tuple) convert'
assert list(range(3)) == [0, 1, 2], 'list(range) convert'
assert list('abc') == ['a', 'b', 'c'], 'list(str) split chars'
assert list('') == [], 'list empty str'

# list copy is independent
orig = [1, 2, 3]
copy = list(orig)
copy.append(4)
assert orig == [1, 2, 3], 'list copy is independent'
assert copy == [1, 2, 3, 4], 'list copy modified'

# === tuple() constructor ===
assert tuple() == (), 'tuple() empty'
assert tuple([1, 2, 3]) == (1, 2, 3), 'tuple(list) convert'
assert tuple((1, 2)) == (1, 2), 'tuple(tuple) copy'
assert tuple(range(3)) == (0, 1, 2), 'tuple(range) convert'
assert tuple('ab') == ('a', 'b'), 'tuple(str) split chars'
assert tuple('') == (), 'tuple empty str'

# === dict() constructor ===
assert dict() == {}, 'dict() empty'
assert dict({1: 2}) == {1: 2}, 'dict(dict) copy'
assert dict({'a': 1, 'b': 2}) == {'a': 1, 'b': 2}, 'dict(dict) multiple keys'

# dict copy is independent
orig_dict = {1: 2}
copy_dict = dict(orig_dict)
copy_dict[3] = 4
assert orig_dict == {1: 2}, 'dict copy is independent'
assert copy_dict == {1: 2, 3: 4}, 'dict copy modified'

# === str() constructor ===
assert str() == '', 'str() empty'
assert str(123) == '123', 'str(int)'
assert str(-42) == '-42', 'str(negative int)'
assert str(0) == '0', 'str(zero)'
assert str(1.5) == '1.5', 'str(float)'
assert str(True) == 'True', 'str(bool True)'
assert str(False) == 'False', 'str(bool False)'
assert str(None) == 'None', 'str(None)'
assert str([1, 2]) == '[1, 2]', 'str(list)'
assert str((1, 2)) == '(1, 2)', 'str(tuple)'
assert str({1: 2}) == '{1: 2}', 'str(dict)'
assert str('hello') == 'hello', 'str(str)'
assert str(b'hi') == "b'hi'", 'str(bytes)'

# === bytes() constructor ===
assert bytes() == b'', 'bytes() empty'
assert bytes(3) == b'\x00\x00\x00', 'bytes(int) zero-filled'
assert bytes(0) == b'', 'bytes(0) empty'
assert bytes(b'hi') == b'hi', 'bytes(bytes) copy'

# === int() constructor ===
assert int() == 0, 'int() default'
assert int(42) == 42, 'int(int)'
assert int(-5) == -5, 'int(negative int)'
assert int(3.7) == 3, 'int(float) truncates down'
assert int(-3.7) == -3, 'int(negative float) truncates toward zero'
assert int(3.0) == 3, 'int(whole float)'
assert int(True) == 1, 'int(True)'
assert int(False) == 0, 'int(False)'

# int() with extreme float values (should clamp to i64 range in Monty)
# Note: Python uses arbitrary precision; Monty clamps to i64
assert isinstance(int(1e18), int), 'int(large float) returns int'
assert isinstance(int(-1e18), int), 'int(large negative float) returns int'
assert int(0.0) == 0, 'int(0.0) is zero'
assert int(-0.0) == 0, 'int(-0.0) is zero'
assert int(0.9) == 0, 'int(0.9) truncates to 0'
assert int(-0.9) == 0, 'int(-0.9) truncates to 0'

# === float() constructor ===
assert float() == 0.0, 'float() default'
assert float(42) == 42.0, 'float(int)'
assert float(-5) == -5.0, 'float(negative int)'
assert float(3.14) == 3.14, 'float(float)'
assert float(True) == 1.0, 'float(True)'
assert float(False) == 0.0, 'float(False)'

# === bool() constructor ===
assert bool() == False, 'bool() default'
assert bool(0) == False, 'bool(0)'
assert bool(1) == True, 'bool(1)'
assert bool(-1) == True, 'bool(-1)'
assert bool(0.0) == False, 'bool(0.0)'
assert bool(1.5) == True, 'bool(1.5)'
assert bool('') == False, 'bool empty str'
assert bool('x') == True, 'bool non-empty str'
assert bool([]) == False, 'bool empty list'
assert bool([1]) == True, 'bool non-empty list'
assert bool(()) == False, 'bool empty tuple'
assert bool((1,)) == True, 'bool non-empty tuple'
assert bool({}) == False, 'bool empty dict'
assert bool({1: 2}) == True, 'bool non-empty dict'
assert bool(None) == False, 'bool(None)'

# === isinstance() ===
assert isinstance(1, int), 'isinstance int'
assert isinstance(1.5, float), 'isinstance float'
assert isinstance(True, bool), 'isinstance bool'
assert isinstance('hello', str), 'isinstance str'
assert isinstance([1, 2], list), 'isinstance list'
assert isinstance((1, 2), tuple), 'isinstance tuple'
assert isinstance({1: 2}, dict), 'isinstance dict'
assert isinstance(b'hi', bytes), 'isinstance bytes'

# isinstance negative cases
assert not isinstance(1, str), 'isinstance int not str'
assert not isinstance('x', int), 'isinstance str not int'
assert not isinstance([], dict), 'isinstance list not dict'

# isinstance with tuple of types
assert isinstance(1, (int, str)), 'isinstance tuple match first'
assert isinstance('x', (int, str)), 'isinstance tuple match second'
assert not isinstance([], (int, str)), 'isinstance tuple no match'
assert isinstance(1, (str, float, int)), 'isinstance tuple match third'

# bool is subtype of int
assert isinstance(True, int), 'bool is instance of int'
assert isinstance(False, int), 'False is instance of int'
assert isinstance(True, (int, str)), 'bool matches int in tuple'

# isinstance with exception types
err = ValueError('test')
assert isinstance(err, ValueError), 'isinstance exception'
assert isinstance(err, Exception), 'isinstance exception base type'
assert not isinstance(err, TypeError), 'isinstance exception wrong type'
assert isinstance(err, (ValueError, TypeError)), 'isinstance exception tuple'

# isinstance with nested tuples
assert isinstance('a', (int, (str, bytes))), 'isinstance nested tuple match'
assert isinstance(1, ((str, float), int)), 'isinstance deeply nested'
assert not isinstance([], (int, (str, bytes))), 'isinstance nested tuple no match'

# NoneType capitalization
assert repr(type(None)) == "<class 'NoneType'>", 'NoneType capitalized'
