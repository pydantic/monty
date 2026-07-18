# === Hash returns int type ===
assert isinstance(hash(42), int)
assert isinstance(hash('hello'), int)
assert isinstance(hash((1, 2, 3)), int)
assert isinstance(hash(3.14), int)

# === Hash consistency for same values ===
assert hash(42) == hash(42)
assert hash(-1) == hash(-1)
assert hash(0) == hash(0)
assert hash('hello') == hash('hello')
assert hash('') == hash('')
assert hash(b'hello') == hash(b'hello')
assert hash(b'') == hash(b'')
assert hash(None) == hash(None)
assert hash(True) == hash(True)
assert hash(False) == hash(False)
assert hash((1, 2, 3)) == hash((1, 2, 3))
assert hash(()) == hash(())
assert hash((1,)) == hash((1,))
assert hash(3.14) == hash(3.14)
assert hash(0.0) == hash(0.0)
assert hash(-0.0) == hash(-0.0)
assert hash(...) == hash(...)

# === Range hash consistency ===
assert hash(range(10)) == hash(range(10))
assert hash(range(0)) == hash(range(0))
assert hash(range(1, 10)) == hash(range(1, 10))
assert hash(range(1, 10, 2)) == hash(range(1, 10, 2))
assert hash(range(-5, 5)) == hash(range(-5, 5))

# === Different range values should hash differently ===
assert hash(range(10)) != hash(range(11))
assert hash(range(10)) != hash(range(1, 10))
assert hash(range(10)) != hash(range(0, 10, 2))
assert hash(range(1, 10, 2)) != hash(range(1, 10, 3))

# === Different values should hash differently ===
assert hash(1) != hash(2)
assert hash('a') != hash('b')
assert hash(b'a') != hash(b'b')
assert hash((1, 2)) != hash((1, 3))
assert hash((1, 2)) != hash((2, 1))
assert hash(True) != hash(False)
assert hash(3.14) != hash(2.71)

# === Type differentiation for clearly different types ===
assert hash(()) != hash('')
assert hash('1') != hash(1)
assert hash(b'1') != hash(1)

# === Nested tuple hashing ===
assert hash((1, (2, 3))) == hash((1, (2, 3)))
assert hash((1, (2, 3))) != hash((1, (2, 4)))
assert hash(((1, 2), (3, 4))) == hash(((1, 2), (3, 4)))

# === String/bytes content equality across representations ===
# Interned strings and heap strings with same content should hash the same
s1 = 'test'
s2 = 'te' + 'st'
assert hash(s1) == hash(s2)

b1 = b'test'
b2 = b'te' + b'st'
assert hash(b1) == hash(b2)


# === Function hashing ===
def f():
    pass


def g():
    pass


assert hash(f) == hash(f)
assert hash(g) == hash(g)
assert hash(f) != hash(g)

# === Builtin function hashing ===
assert hash(len) == hash(len)
assert hash(print) == hash(print)
assert hash(len) != hash(print)

# === Builtin type hashing ===
assert hash(int) == hash(int)
assert hash(str) == hash(str)
assert hash(int) != hash(str)
assert hash(int) != hash(float)

# === Exception type hashing ===
assert hash(ValueError) == hash(ValueError)
assert hash(TypeError) == hash(TypeError)
assert hash(ValueError) != hash(TypeError)

# === Dict key behavior with hashes ===
# Verify that hash consistency works with dict lookups
d = {}
d[42] = 'int'
d['hello'] = 'str'
d[(1, 2)] = 'tuple'
d[range(5)] = 'range'
d[3.14] = 'float'
d[None] = 'none'

assert d[42] == 'int'
assert d['hello'] == 'str'
assert d[(1, 2)] == 'tuple'
assert d[range(5)] == 'range'
assert d[3.14] == 'float'
assert d[None] == 'none'

# === Multiple ranges as dict keys ===
rd = {}
rd[range(5)] = 'a'
rd[range(10)] = 'b'
rd[range(1, 5)] = 'c'
rd[range(0, 5, 2)] = 'd'

assert rd[range(5)] == 'a'
assert rd[range(10)] == 'b'
assert rd[range(1, 5)] == 'c'
assert rd[range(0, 5, 2)] == 'd'
assert len(rd) == 4


# === Functions as dict keys ===
def key_fn():
    pass


fd = {}
fd[key_fn] = 'func_value'
assert fd[key_fn] == 'func_value'

# === Builtins as dict keys ===
bd = {}
bd[len] = 'len_value'
bd[print] = 'print_value'
assert bd[len] == 'len_value'
assert bd[print] == 'print_value'
assert len(bd) == 2

# === Types as dict keys ===
td = {}
td[int] = 'int_type'
td[str] = 'str_type'
td[ValueError] = 'value_error'
assert td[int] == 'int_type'
assert td[str] == 'str_type'
assert td[ValueError] == 'value_error'

# Types which compare equal should hash the same
assert hash(1) == hash(True) and 1 == True, 'int 1 and bool True hash and compare equal'
assert hash(0) == hash(False) and 0 == False, 'int 0 and bool False hash and compare equal'
assert hash(-0.0) == hash(0.0) and -0.0 == 0.0, 'float -0.0 and 0.0 hash and compare equal'
assert hash(1) == hash(1.0) and 1 == 1.0, 'int 1 and float 1.0 hash and compare equal'
assert hash(0) == hash(0.0) and 0 == 0.0, 'int 0 and float 0.0 hash and compare equal'
assert hash(0.0) == hash(False) and 0.0 == False, 'float 0.0 and bool False hash and compare equal'
assert hash(1.0) == hash(True) and 1.0 == True, 'float 1.0 and bool True hash and compare equal'
