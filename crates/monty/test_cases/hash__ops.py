# === Hash returns int type ===
assert isinstance(hash(42), int), 'hash returns int type'
assert isinstance(hash('hello'), int), 'hash of str returns int'
assert isinstance(hash((1, 2, 3)), int), 'hash of tuple returns int'
assert isinstance(hash(3.14), int), 'hash of float returns int'

# === Hash consistency for same values ===
assert hash(42) == hash(42), 'int hash consistent'
assert hash(-1) == hash(-1), 'negative int hash consistent'
assert hash(0) == hash(0), 'zero hash consistent'
assert hash('hello') == hash('hello'), 'str hash consistent'
assert hash('') == hash(''), 'empty str hash consistent'
assert hash(b'hello') == hash(b'hello'), 'bytes hash consistent'
assert hash(b'') == hash(b''), 'empty bytes hash consistent'
assert hash(None) == hash(None), 'None hash consistent'
assert hash(True) == hash(True), 'True hash consistent'
assert hash(False) == hash(False), 'False hash consistent'
assert hash((1, 2, 3)) == hash((1, 2, 3)), 'tuple hash consistent'
assert hash(()) == hash(()), 'empty tuple hash consistent'
assert hash((1,)) == hash((1,)), 'single element tuple hash consistent'
assert hash(3.14) == hash(3.14), 'float hash consistent'
assert hash(0.0) == hash(0.0), 'zero float hash consistent'
assert hash(-0.0) == hash(-0.0), 'negative zero float hash consistent'
assert hash(...) == hash(...), 'ellipsis hash consistent'

# === Range hash consistency ===
assert hash(range(10)) == hash(range(10)), 'range hash consistent'
assert hash(range(0)) == hash(range(0)), 'empty range hash consistent'
assert hash(range(1, 10)) == hash(range(1, 10)), 'range with start hash consistent'
assert hash(range(1, 10, 2)) == hash(range(1, 10, 2)), 'range with step hash consistent'
assert hash(range(-5, 5)) == hash(range(-5, 5)), 'negative start range hash consistent'

# === Different range values should hash differently ===
assert hash(range(10)) != hash(range(11)), 'different range stop hashes differently'
assert hash(range(10)) != hash(range(1, 10)), 'range with different start hashes differently'
assert hash(range(10)) != hash(range(0, 10, 2)), 'range with step hashes differently'
assert hash(range(1, 10, 2)) != hash(range(1, 10, 3)), 'different steps hash differently'

# === Different values should hash differently ===
assert hash(1) != hash(2), 'different ints hash differently'
assert hash('a') != hash('b'), 'different strs hash differently'
assert hash(b'a') != hash(b'b'), 'different bytes hash differently'
assert hash((1, 2)) != hash((1, 3)), 'different tuples hash differently'
assert hash((1, 2)) != hash((2, 1)), 'tuple order matters for hash'
assert hash(True) != hash(False), 'True and False hash differently'
assert hash(3.14) != hash(2.71), 'different floats hash differently'

# === Type differentiation for clearly different types ===
assert hash(()) != hash(''), 'empty tuple and empty str hash differently'
assert hash('1') != hash(1), 'str "1" and int 1 hash differently'
assert hash(b'1') != hash(1), 'bytes b"1" and int 1 hash differently'

# === Nested tuple hashing ===
assert hash((1, (2, 3))) == hash((1, (2, 3))), 'nested tuple hash consistent'
assert hash((1, (2, 3))) != hash((1, (2, 4))), 'nested tuples with different inner values hash differently'
assert hash(((1, 2), (3, 4))) == hash(((1, 2), (3, 4))), 'tuple of tuples hash consistent'

# === String/bytes content equality across representations ===
# Interned strings and heap strings with same content should hash the same
s1 = 'test'
s2 = 'te' + 'st'
assert hash(s1) == hash(s2), 'concatenated string hashes same as literal'

b1 = b'test'
b2 = b'te' + b'st'
assert hash(b1) == hash(b2), 'concatenated bytes hashes same as literal'


# === Function hashing ===
def f():
    pass


def g():
    pass


assert hash(f) == hash(f), 'function hash consistent'
assert hash(g) == hash(g), 'different function hash consistent'
assert hash(f) != hash(g), 'different functions hash differently'

# === Builtin function hashing ===
assert hash(len) == hash(len), 'builtin hash consistent'
assert hash(print) == hash(print), 'print builtin hash consistent'
assert hash(len) != hash(print), 'different builtins hash differently'

# === Builtin type hashing ===
assert hash(int) == hash(int), 'int type hash consistent'
assert hash(str) == hash(str), 'str type hash consistent'
assert hash(int) != hash(str), 'different types hash differently'
assert hash(int) != hash(float), 'int and float types hash differently'

# === Exception type hashing ===
assert hash(ValueError) == hash(ValueError), 'exception type hash consistent'
assert hash(TypeError) == hash(TypeError), 'TypeError hash consistent'
assert hash(ValueError) != hash(TypeError), 'different exception types hash differently'

# === Dict key behavior with hashes ===
# Verify that hash consistency works with dict lookups
d = {}
d[42] = 'int'
d['hello'] = 'str'
d[(1, 2)] = 'tuple'
d[range(5)] = 'range'
d[3.14] = 'float'
d[None] = 'none'

assert d[42] == 'int', 'int dict key works'
assert d['hello'] == 'str', 'str dict key works'
assert d[(1, 2)] == 'tuple', 'tuple dict key works'
assert d[range(5)] == 'range', 'range dict key works'
assert d[3.14] == 'float', 'float dict key works'
assert d[None] == 'none', 'None dict key works'

# === Multiple ranges as dict keys ===
rd = {}
rd[range(5)] = 'a'
rd[range(10)] = 'b'
rd[range(1, 5)] = 'c'
rd[range(0, 5, 2)] = 'd'

assert rd[range(5)] == 'a', 'range(5) key retrieval'
assert rd[range(10)] == 'b', 'range(10) key retrieval'
assert rd[range(1, 5)] == 'c', 'range(1,5) key retrieval'
assert rd[range(0, 5, 2)] == 'd', 'range with step key retrieval'
assert len(rd) == 4, 'all ranges stored as distinct keys'


# === Functions as dict keys ===
def key_fn():
    pass


fd = {}
fd[key_fn] = 'func_value'
assert fd[key_fn] == 'func_value', 'function as dict key works'

# === Builtins as dict keys ===
bd = {}
bd[len] = 'len_value'
bd[print] = 'print_value'
assert bd[len] == 'len_value', 'builtin len as dict key'
assert bd[print] == 'print_value', 'builtin print as dict key'
assert len(bd) == 2, 'different builtins are distinct keys'

# === Types as dict keys ===
td = {}
td[int] = 'int_type'
td[str] = 'str_type'
td[ValueError] = 'value_error'
assert td[int] == 'int_type', 'int type as dict key'
assert td[str] == 'str_type', 'str type as dict key'
assert td[ValueError] == 'value_error', 'exception type as dict key'
