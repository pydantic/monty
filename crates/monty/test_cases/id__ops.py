# === id() returns int type ===
assert isinstance(id(None), int)
assert isinstance(id([]), int)
assert isinstance(id('hello'), int)
assert isinstance(id(42), int)

# === Identity operator (is) ===
assert (True is True) == True
assert (False is False) == True
assert (None is None) == True
assert (... is ...) == True

# === Identity operator (is not) ===
assert (True is not True) == False
assert (True is not False) == True

# === Singleton identity ===
assert id(None) == id(None)
assert id(True) == id(True)
assert id(False) == id(False)
assert id(...) == id(...)

# bool and int are distinct
assert id(True) != id(1)
assert id(False) != id(0)

# distinct singletons
assert id(None) != id(True)
assert id(None) != id(False)
assert id(None) != id(...)

# === Integer identity ===
assert id(10) != id(20)

# === Float identity ===
assert id(1.0) != id(2.0)

# === List assignment shares identity ===
lst = [1, 2]
ref = lst
assert id(lst) == id(ref)
assert lst is ref

# === Variable identity is stable ===
lst = [1, 2]
assert id(lst) == id(lst)

# === List mutation preserves identity ===
a = [1, 2]
b = a
b.append(3)
assert a is b

# === Mixed types have distinct ids ===
assert id(1) != id('1')

# === Tuple singleton is guaranteed to have a unique id ===
assert id([]) != id(())
assert id({}) != id(())
assert id(1) != id(())

# === Multiple refs share id ===
x = [1, 2]
y = x
z = y
assert id(x) == id(y)
assert id(y) == id(z)

# === String assignment shares identity ===
s = 'hello'
r = s
assert id(s) == id(r)

# === Bytes assignment shares identity ===
b = b'hello'
r = b
assert id(b) == id(r)

# === Tuple assignment shares identity ===
t = (1, 2)
r = t
assert id(t) == id(r)

# === Boolean is tests ===
assert (True is True) == True
assert (False is False) == True

# === Array is test ===
a = [1, 2]
b = a
assert (a is b) == True
assert (a is [1, 2]) == False

# === None is tests ===
x = None
assert (x is None) == True
assert (1 is None) == False
