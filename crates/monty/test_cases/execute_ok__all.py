# === Basic arithmetic ===
assert 1 + 1 == 2
assert 'a' + 'b' == 'ab'

# === Equality tests ===
assert (1 == 1) == True
assert (1 == 2) == False
assert ('a' == 'a') == True
assert ('a' == 'b') == False
assert ([1, 2] == [1, 2]) == True
assert ((1, 2) == (1, 2)) == True
assert (b'hello' == b'hello') == True

# === Boolean repr/str ===
assert repr(True) == 'True'
assert str(True) == 'True'
assert repr(False) == 'False'
assert str(False) == 'False'

# === None repr/str ===
assert repr(None) == 'None'
assert str(None) == 'None'

# === Ellipsis repr/str ===
assert repr(...) == 'Ellipsis'
assert str(...) == 'Ellipsis'

# === List repr/str ===
assert repr([1, 2]) == '[1, 2]'
assert str([1, 2]) == '[1, 2]'

# === Discard expression result ===
a = 1
[1, 2, 3]  # this list is created and discarded
assert a == 1

# === Shared list append ===
a = [1]
b = a
b.append(2)
assert len(a) == 2

# === For loop string append ===
v = ''
for i in range(1000):
    if i % 13 == 0:
        v = v + 'x'
assert len(v) == 77

v = ''
for i in range(1000):
    if i % 13 == 0:
        v += 'x'
assert len(v) == 77
