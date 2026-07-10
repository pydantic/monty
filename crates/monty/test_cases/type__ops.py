# === type() function ===
assert type(1) == int
assert type(1.5) == float
assert type(True) == bool
assert type('hello') == str
assert type([1, 2]) == list
assert type((1, 2)) == tuple
assert type({1: 2}) == dict
assert type(b'hi') == bytes
assert type(None) == type(None)

# === type() inequality ===
assert type(1) != str
assert type([]) != tuple
assert type({}) != list
assert type(1) != float

# === type repr ===
assert repr(int) == "<class 'int'>"
assert repr(float) == "<class 'float'>"
assert repr(bool) == "<class 'bool'>"
assert repr(str) == "<class 'str'>"
assert repr(list) == "<class 'list'>"
assert repr(tuple) == "<class 'tuple'>"
assert repr(dict) == "<class 'dict'>"
assert repr(bytes) == "<class 'bytes'>"

# === type identity ===
assert int is int
assert str is str
assert list is list
assert type(1) is int
assert type('') is str
assert type([]) is list

# === list() constructor ===
assert list() == []
assert list([1, 2, 3]) == [1, 2, 3]
assert list((1, 2, 3)) == [1, 2, 3]
assert list(range(3)) == [0, 1, 2]
assert list('abc') == ['a', 'b', 'c']
assert list('') == []

# list copy is independent
orig = [1, 2, 3]
copy = list(orig)
copy.append(4)
assert orig == [1, 2, 3]
assert copy == [1, 2, 3, 4]

# === tuple() constructor ===
assert tuple() == ()
assert tuple([1, 2, 3]) == (1, 2, 3)
assert tuple((1, 2)) == (1, 2)
assert tuple(range(3)) == (0, 1, 2)
assert tuple('ab') == ('a', 'b')
assert tuple('') == ()

# === dict() constructor ===
assert dict() == {}
assert dict({1: 2}) == {1: 2}
assert dict({'a': 1, 'b': 2}) == {'a': 1, 'b': 2}

# dict copy is independent
orig_dict = {1: 2}
copy_dict = dict(orig_dict)
copy_dict[3] = 4
assert orig_dict == {1: 2}
assert copy_dict == {1: 2, 3: 4}
assert dict([('a', 1), ('b', 2)]) == {'a': 1, 'b': 2}
assert dict((('a', 1), ('b', 2))) == {'a': 1, 'b': 2}

headers = ['a', 'b']
row_data = [1, 2]
assert dict(zip(headers, row_data)) == {'a': 1, 'b': 2}
assert dict(zip(['a', 'b'], [1])) == {'a': 1}

assert dict(a=1, b=2) == {'a': 1, 'b': 2}
assert dict([('a', 1)], b=2) == {'a': 1, 'b': 2}
assert dict([('a', 1)], a=2) == {'a': 2}

# === str() constructor ===
assert str() == ''
assert str(123) == '123'
assert str(-42) == '-42'
assert str(0) == '0'
assert str(1.5) == '1.5'
assert str(True) == 'True'
assert str(False) == 'False'
assert str(None) == 'None'
assert str([1, 2]) == '[1, 2]'
assert str((1, 2)) == '(1, 2)'
assert str({1: 2}) == '{1: 2}'
assert str('hello') == 'hello'
assert str(b'hi') == "b'hi'"

# === bytes() constructor ===
assert bytes() == b''
assert bytes(3) == b'\x00\x00\x00'
assert bytes(0) == b''
assert bytes(b'hi') == b'hi'

# === int() constructor ===
assert int() == 0
assert int(42) == 42
assert int(-5) == -5
assert int(3.7) == 3
assert int(-3.7) == -3
assert int(3.0) == 3
assert int(True) == 1
assert int(False) == 0
x = 12345678901234567890
assert int(x) is x

# int() with extreme float values (should clamp to i64 range in Monty)
# Note: Python uses arbitrary precision; Monty clamps to i64
assert isinstance(int(1e18), int)
assert isinstance(int(-1e18), int)
assert int(0.0) == 0
assert int(-0.0) == 0
assert int(0.9) == 0
assert int(-0.9) == 0

# === float() constructor ===
assert float() == 0.0
assert float(42) == 42.0
assert float(-5) == -5.0
assert float(3.14) == 3.14
assert float(True) == 1.0
assert float(False) == 0.0

# === bool() constructor ===
assert bool() == False
assert bool(0) == False
assert bool(1) == True
assert bool(-1) == True
assert bool(0.0) == False
assert bool(1.5) == True
assert bool('') == False
assert bool('x') == True
assert bool([]) == False
assert bool([1]) == True
assert bool(()) == False
assert bool((1,)) == True
assert bool({}) == False
assert bool({1: 2}) == True
assert bool(None) == False

# === isinstance() ===
assert isinstance(1, int)
assert isinstance(1.5, float)
assert isinstance(True, bool)
assert isinstance('hello', str)
assert isinstance([1, 2], list)
assert isinstance((1, 2), tuple)
assert isinstance({1: 2}, dict)
assert isinstance(b'hi', bytes)

# isinstance negative cases
assert not isinstance(1, str), 'isinstance int not str'
assert not isinstance('x', int), 'isinstance str not int'
assert not isinstance([], dict), 'isinstance list not dict'

# isinstance with tuple of types
assert isinstance(1, (int, str))
assert isinstance('x', (int, str))
assert not isinstance([], (int, str)), 'isinstance tuple no match'
assert isinstance(1, (str, float, int))

# bool is subtype of int
assert isinstance(True, int)
assert isinstance(False, int)
assert isinstance(True, (int, str))

# isinstance with exception types
err = ValueError('test')
assert isinstance(err, ValueError)
assert isinstance(err, Exception)
assert not isinstance(err, TypeError), 'isinstance exception wrong type'
assert isinstance(err, (ValueError, TypeError))

# isinstance with nested tuples
assert isinstance('a', (int, (str, bytes)))
assert isinstance(1, ((str, float), int))
assert not isinstance([], (int, (str, bytes))), 'isinstance nested tuple no match'

# NoneType capitalization
assert repr(type(None)) == "<class 'NoneType'>"

# === type().__name__ ===
assert type(42).__name__ == 'int'
assert type('hello').__name__ == 'str'
assert type(True).__name__ == 'bool'
assert type(None).__name__ == 'NoneType'
assert type([1, 2]).__name__ == 'list'
assert type({'a': 1}).__name__ == 'dict'

# type().__name__ for exceptions
try:
    raise ValueError('test')
except ValueError as e:
    assert type(e).__name__ == 'ValueError'
