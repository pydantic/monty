# === Bytes length ===
assert len(b'') == 0, 'len empty'
assert len(b'hello') == 5, 'len basic'

# === Bytes repr/str ===
assert repr(b'hello') == "b'hello'", 'bytes repr'
assert str(b'hello') == "b'hello'", 'bytes str'

# === Various bytes repr cases ===
assert repr(b'') == "b''", 'empty bytes repr'
assert repr(b"it's") == 'b"it\'s"', 'single quote bytes repr'
assert repr(b'l1\nl2') == "b'l1\\nl2'", 'newline bytes repr'
assert repr(b'col1\tcol2') == "b'col1\\tcol2'", 'tab bytes repr'
assert repr(b'\x00\xff') == "b'\\x00\\xff'", 'non-printable bytes repr'
assert repr(b'back\\slash') == "b'back\\\\slash'", 'backslash bytes repr'

# === Bytes repetition (*) ===
assert b'ab' * 3 == b'ababab', 'bytes mult int'
assert 3 * b'ab' == b'ababab', 'int mult bytes'
assert b'x' * 0 == b'', 'bytes mult zero'
assert b'x' * -1 == b'', 'bytes mult negative'
assert b'' * 5 == b'', 'empty bytes mult'
assert b'ab' * 1 == b'ab', 'bytes mult one'

# === Bytes indexing (getitem) ===
# Basic indexing - returns integer byte values
assert b'hello'[0] == 104, 'bytes getitem index 0 (h=104)'
assert b'hello'[1] == 101, 'bytes getitem index 1 (e=101)'
assert b'hello'[4] == 111, 'bytes getitem last index (o=111)'

# Negative indexing
assert b'hello'[-1] == 111, 'bytes getitem -1 (o=111)'
assert b'hello'[-2] == 108, 'bytes getitem -2 (l=108)'
assert b'hello'[-5] == 104, 'bytes getitem -5 (h=104)'

# Single byte
assert b'x'[0] == 120, 'bytes getitem single byte at 0'
assert b'x'[-1] == 120, 'bytes getitem single byte at -1'

# ASCII printable range
assert b' '[0] == 32, 'bytes getitem space (32)'
assert b'~'[0] == 126, 'bytes getitem tilde (126)'

# Non-printable bytes
assert b'\x00'[0] == 0, 'bytes getitem null byte'
assert b'\xff'[0] == 255, 'bytes getitem 0xff'
assert b'\n'[0] == 10, 'bytes getitem newline'
assert b'\t'[0] == 9, 'bytes getitem tab'

# Heap-allocated bytes
b = bytes(b'abc')
assert b[0] == 97, 'heap bytes getitem 0'
assert b[1] == 98, 'heap bytes getitem 1'
assert b[-1] == 99, 'heap bytes negative getitem'

# Variable index
b = b'xyz'
i = 1
assert b[i] == 121, 'bytes getitem with variable index'

# Verify return type is int
val = b'A'[0]
assert type(val) == int, 'bytes getitem returns int'
assert val == 65, 'bytes getitem value is correct'

# Bool indices (True=1, False=0)
b = b'abc'
assert b[False] == 97, 'bytes getitem with False'
assert b[True] == 98, 'bytes getitem with True'
