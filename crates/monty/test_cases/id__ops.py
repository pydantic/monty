# === id() returns int type ===
assert isinstance(id(None), int), 'id returns int type'
assert isinstance(id([]), int), 'id of list returns int'
assert isinstance(id('hello'), int), 'id of str returns int'
assert isinstance(id(42), int), 'id of int returns int'

# === Identity operator (is) ===
assert (True is True) == True, 'is True'
assert (False is False) == True, 'is False'
assert (None is None) == True, 'is None'
assert (... is ...) == True, 'is Ellipsis'

# === Identity operator (is not) ===
assert (True is not True) == False, 'is not True'
assert (True is not False) == True, 'is not False'

# === Singleton identity ===
assert id(None) == id(None), 'None singleton'
assert id(True) == id(True), 'True singleton'
assert id(False) == id(False), 'False singleton'
assert id(...) == id(...), 'Ellipsis singleton'

# bool and int are distinct
assert id(True) != id(1), 'True is not 1'
assert id(False) != id(0), 'False is not 0'

# distinct singletons
assert id(None) != id(True), 'None is not True'
assert id(None) != id(False), 'None is not False'
assert id(None) != id(...), 'None is not Ellipsis'

# === Integer identity ===
assert id(10) != id(20), 'different ints distinct'

# === Float identity ===
assert id(1.0) != id(2.0), 'different floats distinct'

# === List assignment shares identity ===
lst = [1, 2]
ref = lst
assert id(lst) == id(ref), 'list assignment shared'
assert lst is ref, 'list is same'

# === Variable identity is stable ===
lst = [1, 2]
assert id(lst) == id(lst), 'var id stable'

# === List mutation preserves identity ===
a = [1, 2]
b = a
b.append(3)
assert a is b, 'list mutate preserves identity'

# === Mixed types have distinct ids ===
assert id(1) != id('1'), 'int vs str distinct'

# === Tuple singleton is guaranteed to have a unique id ===
assert id([]) != id(()), 'list vs tuple singleton distinct'
assert id({}) != id(()), 'dict vs tuple singleton distinct'
assert id(1) != id(()), 'int vs tuple singleton distinct'

# === Multiple refs share id ===
x = [1, 2]
y = x
z = y
assert id(x) == id(y), 'multiple refs share id xy'
assert id(y) == id(z), 'multiple refs share id yz'

# === String assignment shares identity ===
s = 'hello'
r = s
assert id(s) == id(r), 'str assignment shared'

# === Bytes assignment shares identity ===
b = b'hello'
r = b
assert id(b) == id(r), 'bytes assignment shared'

# === Tuple assignment shares identity ===
t = (1, 2)
r = t
assert id(t) == id(r), 'tuple assignment shared'

# === Boolean is tests ===
assert (True is True) == True, 'bool is test'
assert (False is False) == True, 'bool is test 2'

# === Array is test ===
a = [1, 2]
b = a
assert (a is b) == True, 'array is test'
assert (a is [1, 2]) == False, 'array is new literal'

# === None is tests ===
x = None
assert (x is None) == True, 'var is None'
assert (1 is None) == False, 'int is not None'
