# === del variable (becomes unbound local) ===
x = 42
del x
try:
    x
    assert False, 'expected error after del'
except NameError as exc:
    assert str(exc) == "name 'x' is not defined", f'wrong msg: {exc}'

# === del dict key ===
d = {'a': 1, 'b': 2, 'c': 3}
del d['b']
assert d == {'a': 1, 'c': 3}, f'dict after del: {d}'

# === del dict key (variable index) ===
d2 = {'x': 10, 'y': 20}
key = 'x'
del d2[key]
assert d2 == {'y': 20}, f'dict after del with var key: {d2}'

# === del list item ===
lst = [1, 2, 3, 4, 5]
del lst[1]
assert lst == [1, 3, 4, 5], f'list after del: {lst}'

# === del list negative index ===
lst2 = [10, 20, 30]
del lst2[-1]
assert lst2 == [10, 20], f'list after del[-1]: {lst2}'

# === del multiple targets ===
a = 1
b = 2
c = 3
del a, b
try:
    a
    assert False, 'expected error for a'
except NameError as exc:
    assert str(exc) == "name 'a' is not defined", f'wrong msg: {exc}'
try:
    b
    assert False, 'expected error for b'
except NameError as exc:
    assert str(exc) == "name 'b' is not defined", f'wrong msg: {exc}'
assert c == 3, 'c should be unchanged'

# === del missing global raises NameError ===
try:
    del missing
    assert False, 'expected NameError for missing'
except NameError as exc:
    assert str(exc) == "name 'missing' is not defined", f'wrong msg: {exc}'

# === del duplicated target raises on second delete ===
dup = 1
try:
    del dup, dup
    assert False, 'expected NameError for repeated delete target'
except NameError as exc:
    assert str(exc) == "name 'dup' is not defined", f'wrong msg: {exc}'

try:
    dup
    assert False, 'expected dup to remain deleted'
except NameError as exc:
    assert str(exc) == "name 'dup' is not defined", f'wrong msg: {exc}'


# === del unbound local raises UnboundLocalError ===
def delete_unbound_local():
    try:
        del local_name
        assert False, 'expected UnboundLocalError for local delete'
    except UnboundLocalError as exc:
        return str(exc)

    local_name = 1


assert delete_unbound_local() == (
    "cannot access local variable 'local_name' where it is not associated with a value"
), 'wrong unbound-local delete message'


# === del captured local keeps cell unbound ===
def delete_captured_local():
    x = 1

    def inner():
        return x

    del x

    try:
        x
        assert False, 'expected UnboundLocalError after deleting captured local'
    except UnboundLocalError as exc:
        outer_msg = str(exc)

    try:
        inner()
        assert False, 'expected NameError from closure after delete'
    except NameError as exc:
        inner_msg = str(exc)

    return outer_msg, inner_msg


assert delete_captured_local() == (
    "cannot access local variable 'x' where it is not associated with a value",
    "cannot access free variable 'x' where it is not associated with a value in enclosing scope",
), 'wrong messages after deleting captured local'


# === del nonlocal keeps outer cell unbound ===
def delete_nonlocal_binding():
    x = 1

    def delete_x():
        nonlocal x
        del x

    def read_x():
        return x

    delete_x()

    try:
        x
        assert False, 'expected UnboundLocalError in outer scope after nonlocal del'
    except UnboundLocalError as exc:
        outer_msg = str(exc)

    try:
        read_x()
        assert False, 'expected NameError in sibling closure after nonlocal del'
    except NameError as exc:
        inner_msg = str(exc)

    return outer_msg, inner_msg


assert delete_nonlocal_binding() == (
    "cannot access local variable 'x' where it is not associated with a value",
    "cannot access free variable 'x' where it is not associated with a value in enclosing scope",
), 'wrong messages after deleting nonlocal binding'

# === del and re-assign ===
val = 'hello'
del val
val = 'world'
assert val == 'world', f'reassigned value: {val}'

# === del in loop ===
d3 = {'a': 1, 'b': 2, 'c': 3}
keys_to_remove = ['a', 'c']
for k in keys_to_remove:
    del d3[k]
assert d3 == {'b': 2}, f'dict after loop del: {d3}'

# === del list first element repeatedly ===
lst3 = [1, 2, 3]
del lst3[0]
del lst3[0]
assert lst3 == [3], f'list after repeated del[0]: {lst3}'

# === del dict integer key ===
d4 = {1: 'one', 2: 'two', 3: 'three'}
del d4[2]
assert d4 == {1: 'one', 3: 'three'}, f'dict int key del: {d4}'

# === del list with len check ===
lst4 = [10, 20, 30, 40]
assert len(lst4) == 4, 'initial len'
del lst4[2]
assert len(lst4) == 3, 'len after del'
assert lst4 == [10, 20, 40], f'list after del[2]: {lst4}'

# === del dict missing key raises KeyError ===
d5 = {'a': 1}
try:
    del d5['b']
    assert False, 'expected KeyError'
except KeyError:
    pass

# === del list out of range raises IndexError ===
lst5 = [1, 2]
try:
    del lst5[10]
    assert False, 'expected IndexError'
except IndexError:
    pass

# === del on non-subscriptable type raises TypeError ===
t = (1, 2, 3)
try:
    del t[0]
    assert False, 'expected TypeError'
except TypeError as e:
    assert str(e) == "'tuple' object does not support item deletion", f'wrong msg: {e}'
