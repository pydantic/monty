# === Bytes length ===
assert len(b'') == 0
assert len(b'hello') == 5

# === Bytes repr/str ===
assert repr(b'hello') == "b'hello'"
assert str(b'hello') == "b'hello'"

# === Various bytes repr cases ===
assert repr(b'') == "b''"
assert repr(b"it's") == 'b"it\'s"'
assert repr(b'l1\nl2') == "b'l1\\nl2'"
assert repr(b'col1\tcol2') == "b'col1\\tcol2'"
assert repr(b'\x00\xff') == "b'\\x00\\xff'"
assert repr(b'back\\slash') == "b'back\\\\slash'"

# === Bytes repetition (*) ===
assert b'ab' * 3 == b'ababab'
assert 3 * b'ab' == b'ababab'
assert b'x' * 0 == b''
assert b'x' * -1 == b''
assert b'' * 5 == b''
assert b'ab' * 1 == b'ab'

# === Bytes indexing (getitem) ===
# Basic indexing - returns integer byte values
assert b'hello'[0] == 104
assert b'hello'[1] == 101
assert b'hello'[4] == 111

# Negative indexing
assert b'hello'[-1] == 111
assert b'hello'[-2] == 108
assert b'hello'[-5] == 104

# Single byte
assert b'x'[0] == 120
assert b'x'[-1] == 120

# ASCII printable range
assert b' '[0] == 32
assert b'~'[0] == 126

# Non-printable bytes
assert b'\x00'[0] == 0
assert b'\xff'[0] == 255
assert b'\n'[0] == 10
assert b'\t'[0] == 9

# Heap-allocated bytes
b = bytes(b'abc')
assert b[0] == 97
assert b[1] == 98
assert b[-1] == 99

# Variable index
b = b'xyz'
i = 1
assert b[i] == 121

# Verify return type is int
val = b'A'[0]
assert type(val) == int
assert val == 65

# Bool indices (True=1, False=0)
b = b'abc'
assert b[False] == 97
assert b[True] == 98

# === Bytes comparisons ===
assert b'abc' < b'abd'
assert b'abd' > b'abc'
assert b'abc' <= b'abc'
assert b'abc' <= b'abd'
assert b'abd' >= b'abd'
assert b'abd' >= b'abc'

# Different lengths
assert b'ab' < b'abc'
assert b'' < b'a'
assert b'abc' > b'ab'

# Non-ASCII byte values
assert b'\x00' < b'\xff'
assert b'\xfe' < b'\xff'

# Sorting
assert sorted([b'c', b'a', b'b']) == [b'a', b'b', b'c']
assert sorted([b'bb', b'a', b'ba']) == [b'a', b'ba', b'bb']

# === bytes() constructor with keyword argument ===
assert bytes(source=b'hello') == b'hello'
assert bytes(source=3) == b'\x00\x00\x00'

# bytes() constructor error cases
try:
    bytes(wrong=3)
    assert False, 'bytes wrong kwarg should raise'
except TypeError as e:
    assert str(e) == "bytes() got an unexpected keyword argument 'wrong'", f'wrong: {e}'

try:
    bytes(3, source=3)
    assert False, 'bytes pos + kwarg should raise'
except TypeError as e:
    assert str(e) == "argument for bytes() given by name ('source') and position (1)", f'dup: {e}'
